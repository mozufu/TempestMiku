use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;
use tm_core::{CancellationToken, CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};
use tm_drive::SharedDriveStore;
use tm_host::{
    ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants, DefaultDenyApprovalPolicy, GrantDoc,
    HostError, HostEventSink, HostFn, HostRegistry, InvocationCtx, LinkedFolders,
    NoopHostEventSink, ResourceRegistry, ToolDocs, register_p0_linked_folder_functions,
};

use crate::{
    Interpreter, RuntimeError, RuntimeLimits, RuntimeOutput, RuntimeResult,
    batch::binding_usage_bounded, catalog_from_registry,
};

#[derive(Clone)]
pub struct TmSandboxOptions {
    pub artifact_root: PathBuf,
    pub session_id: String,
    pub actor_id: Option<String>,
    pub session_scope: Option<String>,
    pub http_allowlist: BTreeMap<String, String>,
    pub host_registry: HostRegistry,
    pub resource_registry: ResourceRegistry,
    pub grants: CapabilityGrants,
    pub linked_folders: Option<LinkedFolders>,
    pub drive_store: Option<SharedDriveStore>,
    pub approval_policy: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
    pub proc_run_timeout: Duration,
    pub host_event_sink: Arc<dyn HostEventSink>,
    pub limits: RuntimeLimits,
    pub cancellation: Option<Arc<dyn CancellationToken>>,
}

impl Default for TmSandboxOptions {
    fn default() -> Self {
        Self {
            artifact_root: tm_artifacts::default_root(),
            session_id: "default".into(),
            actor_id: None,
            session_scope: None,
            http_allowlist: BTreeMap::new(),
            host_registry: HostRegistry::new(),
            resource_registry: ResourceRegistry::new(),
            // Registration and sandbox construction never grant model authority. Callers must
            // supply every externally authoritative capability for this exact turn/actor.
            grants: CapabilityGrants::default(),
            linked_folders: None,
            drive_store: None,
            approval_policy: Arc::new(DefaultDenyApprovalPolicy),
            approval_timeout: Duration::from_secs(60),
            proc_run_timeout: Duration::from_millis(180_000),
            host_event_sink: Arc::new(NoopHostEventSink),
            limits: RuntimeLimits::default(),
            cancellation: None,
        }
    }
}

#[derive(Clone)]
pub struct TmSandbox {
    options: TmSandboxOptions,
}
impl TmSandbox {
    pub fn new(options: TmSandboxOptions) -> Self {
        Self { options }
    }
}

#[async_trait]
impl Sandbox for TmSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        let artifact_root = self.options.artifact_root.clone();
        let artifact_session_id = self.options.session_id.clone();
        let artifacts = tokio::task::spawn_blocking(move || {
            ArtifactStore::open(artifact_root, artifact_session_id)
        })
        .await
        .map_err(|error| tm_core::Error::Sandbox(format!("artifact open task failed: {error}")))?
        .map_err(|error| tm_core::Error::Sandbox(error.to_string()))?;
        let mut registry = self.options.host_registry.clone();
        registry.register(Arc::new(HttpGetFn::new(
            self.options.http_allowlist.clone(),
        )));
        let mut resources = self.options.resource_registry.clone();
        resources.register(Arc::new(ArtifactResourceHandler::new(artifacts.clone())));
        let linked_folders = self.options.linked_folders.clone().or_else(|| {
            self.options
                .drive_store
                .as_ref()
                .map(|_| LinkedFolders::default())
        });
        if let Some(linked_folders) = linked_folders.clone() {
            register_p0_linked_folder_functions(
                &mut registry,
                &mut resources,
                linked_folders,
                artifacts.clone(),
                self.options.proc_run_timeout,
            );
        }
        if let Some(drive_store) = self.options.drive_store.clone() {
            tm_drive::register_drive_functions(
                &mut registry,
                &mut resources,
                drive_store,
                linked_folders.clone(),
            );
        }
        let mut invocation = InvocationCtx::with_approvals(
            self.options.grants.clone(),
            self.options.approval_policy.clone(),
            self.options.approval_timeout,
        )
        .with_session_id(self.options.session_id.clone())
        .with_actor_id(self.options.actor_id.clone())
        .with_event_sink(self.options.host_event_sink.clone());
        if let Some(scope) = self.options.session_scope.clone() {
            invocation = invocation.with_session_scope(scope);
        }
        let resource_schemes = resources
            .capabilities()
            .into_iter()
            .filter_map(|(scheme, capability)| {
                invocation.grants.permits(&capability).then_some(scheme)
            })
            .collect::<Vec<_>>();
        if !resource_schemes.is_empty() {
            invocation.grants = invocation.grants.clone().allow_many([
                "resources.read",
                "resources.preview",
                "resources.list",
            ]);
        }
        let mut catalog_schemes = resource_schemes;
        // `alias:path` is tm's compact linked-folder literal. It is not a ResourceRegistry
        // scheme, so admit only aliases that this server-authoritative session scope can use.
        if let Some(linked_folders) = &linked_folders {
            catalog_schemes.extend(
                linked_folders
                    .policies()
                    .into_iter()
                    .map(|policy| policy.alias)
                    .filter(|alias| invocation.require_linked_alias(alias).is_ok()),
            );
        }
        catalog_schemes.sort();
        catalog_schemes.dedup();
        for operation in [
            ResourceOperation::Read,
            ResourceOperation::Preview,
            ResourceOperation::List,
        ] {
            registry.register(Arc::new(ResourceFn::new(operation, resources.clone())));
        }
        for operation in [
            ArtifactOperation::Put,
            ArtifactOperation::Get,
            ArtifactOperation::Slice,
            ArtifactOperation::List,
        ] {
            registry.register(Arc::new(ArtifactFn::new(operation, artifacts.clone())));
        }
        // Session-local output spilling and granted-catalog inspection are intrinsic tm runtime
        // operations. They do not add host, resource-read, network, or child authority.
        invocation.grants =
            invocation
                .grants
                .clone()
                .allow_many(["artifacts.put", "tools.search", "tools.docs"]);
        if invocation.grants.permits("resources.read:artifact") {
            invocation.grants = invocation.grants.clone().allow_many([
                "artifacts.get",
                "artifacts.slice",
                "artifacts.list",
            ]);
            if !catalog_schemes.iter().any(|scheme| scheme == "artifact") {
                catalog_schemes.push("artifact".into());
            }
        }
        let catalog_registry = Arc::new(registry.clone());
        for operation in [CatalogOperation::Search, CatalogOperation::Docs] {
            registry.register(Arc::new(CatalogFn::new(
                operation,
                Arc::clone(&catalog_registry),
            )));
        }
        let registry = Arc::new(registry);
        let catalog = catalog_from_registry(&registry, &invocation, catalog_schemes);
        Ok(Box::new(TmSession {
            interpreter: Interpreter::new(
                catalog,
                registry,
                invocation,
                self.options.limits.clone(),
            ),
            cancellation: self.options.cancellation.clone(),
            limits: self.options.limits.clone(),
        }))
    }
}

#[derive(Debug, Clone)]
struct HttpGetFn {
    responses: BTreeMap<String, String>,
    docs: ToolDocs,
}

impl HttpGetFn {
    fn new(responses: BTreeMap<String, String>) -> Self {
        Self {
            responses,
            docs: ToolDocs {
                name: "http.get".into(),
                namespace: "http".into(),
                summary: "Fetch a deterministic allowlisted HTTP response".into(),
                description: Some(
                    "Default-deny deterministic fixture access. This is not ambient network egress; production egress remains owned by P9."
                        .into(),
                ),
                signature: "http.get(url: String) -> String".into(),
                args_schema: json!({
                    "type": "object",
                    "required": ["url"],
                    "additionalProperties": false,
                    "properties": { "url": { "type": "string", "format": "uri" } }
                }),
                result_schema: Some(json!({"type": "string"})),
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "network".into(),
                    description: "Deterministic allowlisted fixture access only".into(),
                }],
                sensitive: true,
                approval: "none".into(),
                since: "M1".into(),
                stability: "experimental".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for HttpGetFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| HostError::InvalidArgs("http.get requires a string url".into()))?;
        self.responses
            .get(url)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| HostError::CapabilityDenied("http.get".into()))
    }
}

#[derive(Clone, Copy)]
enum ArtifactOperation {
    Put,
    Get,
    Slice,
    List,
}

struct ArtifactFn {
    operation: ArtifactOperation,
    store: ArtifactStore,
    docs: ToolDocs,
}

impl ArtifactFn {
    fn new(operation: ArtifactOperation, store: ArtifactStore) -> Self {
        let (name, summary, sensitive) = match operation {
            ArtifactOperation::Put => ("artifacts.put", "Store a session artifact", true),
            ArtifactOperation::Get => ("artifacts.get", "Read a session artifact", true),
            ArtifactOperation::Slice => ("artifacts.slice", "Read a selected artifact slice", true),
            ArtifactOperation::List => ("artifacts.list", "List session artifacts", true),
        };
        let (signature, args_schema) = match operation {
            ArtifactOperation::List => (
                "artifacts.list({offset?: number, limit?: number})".into(),
                json!({
                    "type": ["object", "null"],
                    "properties": {
                        "offset": { "type": "integer", "minimum": 0 },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 256 }
                    },
                    "additionalProperties": false
                }),
            ),
            _ => (format!("{name}(args)"), json!({})),
        };
        Self {
            operation,
            store,
            docs: ToolDocs {
                name: name.into(),
                namespace: "artifacts".into(),
                summary: summary.into(),
                description: None,
                signature,
                args_schema,
                result_schema: None,
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "artifact".into(),
                    description: match operation {
                        ArtifactOperation::Get
                        | ArtifactOperation::Slice
                        | ArtifactOperation::List => "Reads require resources.read:artifact",
                        _ => "Session-local artifact operation",
                    }
                    .into(),
                }],
                sensitive,
                approval: "none".into(),
                since: "0.1".into(),
                stability: "stable".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for ArtifactFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        match self.operation {
            ArtifactOperation::Put => {
                let (data, title, mime) = match args {
                    Value::Object(mut fields) if fields.contains_key("data") => {
                        let data = fields.remove("data").unwrap_or(Value::Null);
                        let title = fields
                            .remove("title")
                            .and_then(|value| value.as_str().map(str::to_string));
                        let mime = fields
                            .remove("mime")
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_else(|| "text/plain".into());
                        (data, title, mime)
                    }
                    data => (data, None, "text/plain".into()),
                };
                validate_mime(&mime)?;
                let content = match data {
                    Value::String(content) => content,
                    value => serde_json::to_string_pretty(&value)
                        .map_err(|error| HostError::InvalidArgs(error.to_string()))?,
                };
                let content = tm_memory::redact_dream_text(&content).text;
                let title = title.map(|title| tm_memory::redact_dream_text(&title).text);
                let store = self.store.clone();
                let artifact =
                    tokio::task::spawn_blocking(move || store.put_text(content, title, &mime))
                        .await
                        .map_err(|error| {
                            HostError::HostCall(format!("artifact write task failed: {error}"))
                        })?
                        .map_err(|error| HostError::HostCall(error.to_string()))?;
                serde_json::to_value(artifact)
                    .map_err(|error| HostError::HostCall(error.to_string()))
            }
            ArtifactOperation::Get | ArtifactOperation::Slice => {
                if !ctx.grants.permits("resources.read:artifact") {
                    return Err(HostError::CapabilityDenied(
                        "resources.read:artifact".into(),
                    ));
                }
                let (uri, selector) = artifact_read_args(&args, self.operation)?;
                let store = self.store.clone();
                let content =
                    tokio::task::spawn_blocking(move || store.read(&uri, selector.as_deref()))
                        .await
                        .map_err(|error| {
                            HostError::HostCall(format!("artifact read task failed: {error}"))
                        })?
                        .map_err(|error| HostError::NotFound(error.to_string()))?;
                serde_json::to_value(content)
                    .map_err(|error| HostError::HostCall(error.to_string()))
            }
            ArtifactOperation::List => {
                if !ctx.grants.permits("resources.read:artifact") {
                    return Err(HostError::CapabilityDenied(
                        "resources.read:artifact".into(),
                    ));
                }
                let (offset, limit) = artifact_list_args(&args)?;
                let store = self.store.clone();
                let (items, has_more) =
                    tokio::task::spawn_blocking(move || store.list_page(offset, limit))
                        .await
                        .map_err(|error| {
                            HostError::HostCall(format!("artifact list task failed: {error}"))
                        })?
                        .map_err(|error| HostError::HostCall(error.to_string()))?;
                let next_offset = has_more.then(|| offset.saturating_add(items.len()));
                Ok(json!({
                    "items": items,
                    "offset": offset,
                    "nextOffset": next_offset,
                    "hasMore": has_more,
                }))
            }
        }
    }
}

fn artifact_list_args(args: &Value) -> tm_host::Result<(usize, usize)> {
    const DEFAULT_LIMIT: usize = 100;
    const MAX_LIMIT: usize = 256;
    if args.is_null() {
        return Ok((0, DEFAULT_LIMIT));
    }
    let fields = args
        .as_object()
        .ok_or_else(|| HostError::InvalidArgs("artifact list requires an object or null".into()))?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "offset" | "limit"))
    {
        return Err(HostError::InvalidArgs(
            "artifact list accepts only offset and limit".into(),
        ));
    }
    let parse = |name: &str, default: usize| -> tm_host::Result<usize> {
        let Some(value) = fields.get(name) else {
            return Ok(default);
        };
        let value = value.as_u64().ok_or_else(|| {
            HostError::InvalidArgs(format!(
                "artifact list {name} must be a non-negative integer"
            ))
        })?;
        usize::try_from(value)
            .map_err(|_| HostError::InvalidArgs(format!("artifact list {name} is too large")))
    };
    let offset = parse("offset", 0)?;
    let limit = parse("limit", DEFAULT_LIMIT)?;
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(HostError::InvalidArgs(format!(
            "artifact list limit must be in 1..={MAX_LIMIT}"
        )));
    }
    Ok((offset, limit))
}

fn artifact_read_args(
    args: &Value,
    operation: ArtifactOperation,
) -> tm_host::Result<(String, Option<String>)> {
    let direct = args.as_str().map(str::to_string);
    let fields = args.as_object();
    let uri = direct
        .or_else(|| {
            fields
                .and_then(|fields| fields.get("ref").or_else(|| fields.get("uri")))
                .and_then(|value| match value {
                    Value::String(uri) => Some(uri.clone()),
                    Value::Object(reference) => reference
                        .get("uri")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    _ => None,
                })
        })
        .ok_or_else(|| HostError::InvalidArgs("artifact read requires ref or uri".into()))?;
    let selector = fields
        .and_then(|fields| fields.get("selector"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if matches!(operation, ArtifactOperation::Slice) && selector.is_none() {
        return Err(HostError::InvalidArgs(
            "artifacts.slice requires selector".into(),
        ));
    }
    Ok((uri, selector))
}

fn validate_mime(mime: &str) -> tm_host::Result<()> {
    const MAX_MIME_BYTES: usize = 127;
    let Some((kind, subtype)) = mime.split_once('/') else {
        return Err(HostError::InvalidArgs(
            "artifact MIME must be a type/subtype token".into(),
        ));
    };
    let valid_token = |token: &str| {
        !token.is_empty()
            && token.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    };
    if mime.len() > MAX_MIME_BYTES
        || subtype.contains('/')
        || !valid_token(kind)
        || !valid_token(subtype)
    {
        return Err(HostError::InvalidArgs(
            "artifact MIME must be a bounded ASCII type/subtype token".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CatalogOperation {
    Search,
    Docs,
}

struct CatalogFn {
    operation: CatalogOperation,
    registry: Arc<HostRegistry>,
    docs: ToolDocs,
}

impl CatalogFn {
    fn new(operation: CatalogOperation, registry: Arc<HostRegistry>) -> Self {
        let (name, summary, signature) = match operation {
            CatalogOperation::Search => (
                "tools.search",
                "Search the granted tm effect catalog",
                "tools.search(query)",
            ),
            CatalogOperation::Docs => (
                "tools.docs",
                "Read one tm effect declaration and its host policy metadata",
                "tools.docs(name)",
            ),
        };
        Self {
            operation,
            registry,
            docs: ToolDocs {
                name: name.into(),
                namespace: "tools".into(),
                summary: summary.into(),
                description: None,
                signature: signature.into(),
                args_schema: json!({}),
                result_schema: None,
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "catalog".into(),
                    description: "Catalog inspection does not grant target authority".into(),
                }],
                sensitive: false,
                approval: "none".into(),
                since: "0.1".into(),
                stability: "stable".into(),
            },
        }
    }

    fn docs_value(docs: ToolDocs) -> tm_host::Result<Value> {
        let declaration = format!("eff {} : Json -> Json", docs.name);
        let resumable = docs.approval != "none";
        let mut value =
            serde_json::to_value(docs).map_err(|error| HostError::HostCall(error.to_string()))?;
        let fields = value
            .as_object_mut()
            .expect("ToolDocs always serializes as an object");
        fields.insert("tmDeclaration".into(), Value::String(declaration));
        fields.insert("resumable".into(), Value::Bool(resumable));
        Ok(value)
    }
}

#[async_trait]
impl HostFn for CatalogFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        match self.operation {
            CatalogOperation::Search => {
                let (query, namespace, limit) = match args {
                    Value::String(query) => (query, None, 20),
                    Value::Object(fields) => (
                        fields
                            .get("query")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        fields
                            .get("namespace")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        fields.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize,
                    ),
                    _ => {
                        return Err(HostError::InvalidArgs(
                            "tools.search requires a query string or record".into(),
                        ));
                    }
                };
                serde_json::to_value(self.registry.search(
                    &query,
                    namespace.as_deref(),
                    limit.clamp(1, 100),
                    ctx,
                ))
                .map_err(|error| HostError::HostCall(error.to_string()))
            }
            CatalogOperation::Docs => {
                let name = match &args {
                    Value::String(name) => Some(name.as_str()),
                    Value::Object(fields) => fields.get("name").and_then(Value::as_str),
                    _ => None,
                }
                .ok_or_else(|| HostError::InvalidArgs("tools.docs requires a name".into()))?;
                let docs = if name == self.docs.name {
                    self.docs.clone()
                } else {
                    self.registry.docs(name, ctx)?
                };
                Self::docs_value(docs)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceOperation {
    Read,
    Preview,
    List,
}

struct ResourceFn {
    operation: ResourceOperation,
    resources: ResourceRegistry,
    docs: ToolDocs,
}

impl ResourceFn {
    fn new(operation: ResourceOperation, resources: ResourceRegistry) -> Self {
        let (name, summary) = match operation {
            ResourceOperation::Read => ("resources.read", "Read a registered resource URI"),
            ResourceOperation::Preview => {
                ("resources.preview", "Preview a registered resource URI")
            }
            ResourceOperation::List => ("resources.list", "List registered resources"),
        };
        Self {
            operation,
            resources,
            docs: ToolDocs {
                name: name.into(),
                namespace: "resources".into(),
                summary: summary.into(),
                description: None,
                signature: format!("{name}(uri)"),
                args_schema: json!({}),
                result_schema: None,
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: name.into(),
                }],
                // Resource content and even URI listings may expose private session/user data.
                // This controls trace redaction only; reads remain non-approval operations.
                sensitive: true,
                approval: "none".into(),
                since: "0.1".into(),
                stability: "stable".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for ResourceFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let uri = match &args {
            Value::String(uri) => Some(uri.as_str()),
            Value::Object(fields) => fields.get("uri").and_then(Value::as_str),
            _ => None,
        };
        match self.operation {
            ResourceOperation::Read => {
                let uri = uri
                    .ok_or_else(|| HostError::InvalidArgs("resources.read requires uri".into()))?;
                let selector = args
                    .as_object()
                    .and_then(|fields| fields.get("selector"))
                    .and_then(Value::as_str);
                serde_json::to_value(self.resources.read(uri, selector, ctx).await?)
                    .map_err(|error| HostError::HostCall(error.to_string()))
            }
            ResourceOperation::Preview => {
                let uri = uri.ok_or_else(|| {
                    HostError::InvalidArgs("resources.preview requires uri".into())
                })?;
                serde_json::to_value(self.resources.preview(uri, ctx).await?)
                    .map_err(|error| HostError::HostCall(error.to_string()))
            }
            ResourceOperation::List => {
                let mut entries = self.resources.list(uri, ctx).await?;
                if uri.is_none() {
                    let allowed: BTreeSet<_> = self
                        .resources
                        .capabilities()
                        .into_iter()
                        .filter_map(|(scheme, capability)| {
                            ctx.grants.permits(&capability).then_some(scheme)
                        })
                        .collect();
                    entries.retain(|entry| entry.kind != "scheme" || allowed.contains(&entry.name));
                }
                serde_json::to_value(entries)
                    .map_err(|error| HostError::HostCall(error.to_string()))
            }
        }
    }
}

pub struct TmSession {
    interpreter: Interpreter,
    cancellation: Option<Arc<dyn CancellationToken>>,
    limits: RuntimeLimits,
}

enum EvaluationRace {
    Completed(RuntimeResult<RuntimeOutput>),
    Cancelled,
    TimedOut,
    TerminalPersistenceTimedOut,
}

const TERMINAL_PERSISTENCE_GRACE: Duration = Duration::from_secs(1);

struct BoundedFormatter {
    text: String,
    max_bytes: usize,
}

impl BoundedFormatter {
    fn new(max_bytes: usize) -> Self {
        Self {
            text: String::with_capacity(max_bytes.min(1024)),
            max_bytes,
        }
    }
}

impl fmt::Write for BoundedFormatter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = self.max_bytes.saturating_sub(self.text.len());
        if remaining == 0 {
            return Err(fmt::Error);
        }
        let mut end = remaining.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        if end == value.len() {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

fn bounded_display(value: &impl fmt::Display, max_bytes: usize) -> String {
    let mut output = BoundedFormatter::new(max_bytes);
    let _ = write!(&mut output, "{value}");
    output.text
}

async fn race_evaluation(
    interpreter: &mut Interpreter,
    code: &str,
    budget: CellBudget,
    cancellation: Option<&dyn CancellationToken>,
) -> EvaluationRace {
    let terminal_selected = interpreter.terminal_selected_handle();
    terminal_selected.store(false, std::sync::atomic::Ordering::Release);
    let evaluation = interpreter.eval(code, budget.output_bytes);
    tokio::pin!(evaluation);
    let wall = tokio::time::sleep(Duration::from_millis(budget.wall_ms));
    tokio::pin!(wall);
    if let Some(token) = cancellation {
        tokio::select! {
            result = &mut evaluation => EvaluationRace::Completed(result),
            _ = token.cancelled() => {
                if terminal_selected.load(std::sync::atomic::Ordering::Acquire) {
                    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, &mut evaluation).await {
                        Ok(result) => EvaluationRace::Completed(result),
                        Err(_) => EvaluationRace::TerminalPersistenceTimedOut,
                    }
                } else {
                    EvaluationRace::Cancelled
                }
            },
            _ = &mut wall => {
                if terminal_selected.load(std::sync::atomic::Ordering::Acquire) {
                    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, &mut evaluation).await {
                        Ok(result) => EvaluationRace::Completed(result),
                        Err(_) => EvaluationRace::TerminalPersistenceTimedOut,
                    }
                } else {
                    EvaluationRace::TimedOut
                }
            },
        }
    } else {
        tokio::select! {
            result = &mut evaluation => EvaluationRace::Completed(result),
            _ = &mut wall => {
                if terminal_selected.load(std::sync::atomic::Ordering::Acquire) {
                    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, &mut evaluation).await {
                        Ok(result) => EvaluationRace::Completed(result),
                        Err(_) => EvaluationRace::TerminalPersistenceTimedOut,
                    }
                } else {
                    EvaluationRace::TimedOut
                }
            },
        }
    }
}

async fn cancel_active_bounded(
    interpreter: &mut Interpreter,
    status: &str,
    reason: &str,
) -> Result<()> {
    match tokio::time::timeout(
        TERMINAL_PERSISTENCE_GRACE,
        interpreter.cancel_active_eval(status, reason),
    )
    .await
    {
        Ok(result) => result.map_err(|error| tm_core::Error::Sandbox(error.to_string())),
        Err(_) => {
            interpreter.abandon_active_eval();
            Err(tm_core::Error::Sandbox(
                "terminal event persistence deadline exceeded".into(),
            ))
        }
    }
}

async fn emit_immediate_bounded(
    interpreter: &mut Interpreter,
    code: &str,
    status: &str,
    reason: &str,
) -> Result<()> {
    persist_terminal_bounded(interpreter.emit_immediate_terminal(code, status, reason)).await
}

async fn emit_dependency_failure_bounded(
    interpreter: &mut Interpreter,
    code: &str,
    reason: &str,
) -> Result<()> {
    persist_terminal_bounded(interpreter.emit_dependency_failure(code, reason)).await
}

async fn persist_terminal_bounded(
    persistence: impl Future<Output = RuntimeResult<()>>,
) -> Result<()> {
    match tokio::time::timeout(TERMINAL_PERSISTENCE_GRACE, persistence).await {
        Ok(result) => result.map_err(|error| tm_core::Error::Sandbox(error.to_string())),
        Err(_) => Err(tm_core::Error::Sandbox(
            "terminal event persistence deadline exceeded".into(),
        )),
    }
}

#[async_trait(?Send)]
impl Session for TmSession {
    fn handles_cancellation(&self) -> bool {
        self.cancellation.is_some()
    }

    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput> {
        if budget.wall_ms == 0 {
            emit_immediate_bounded(
                &mut self.interpreter,
                code,
                "timed_out",
                "cell exceeded wall-clock budget",
            )
            .await?;
            return Ok(timeout_output(budget.output_bytes));
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            emit_immediate_bounded(&mut self.interpreter, code, "cancelled", "cell cancelled")
                .await?;
            return Ok(cancelled_output(budget.output_bytes));
        }
        let result = race_evaluation(
            &mut self.interpreter,
            code,
            budget,
            self.cancellation.as_deref(),
        )
        .await;
        match result {
            EvaluationRace::Completed(Ok(output)) => Ok(EvalOutput {
                stdout: output.stdout,
                result: Some(output.value.to_json()),
                error: None,
            }),
            EvaluationRace::Completed(Err(RuntimeError::Persistence(error))) => {
                self.interpreter.abandon_active_eval();
                Err(tm_core::Error::Sandbox(error))
            }
            EvaluationRace::Completed(Err(error)) => Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(bounded_display(&error, budget.output_bytes)),
            }),
            EvaluationRace::Cancelled => {
                cancel_active_bounded(&mut self.interpreter, "cancelled", "cell cancelled").await?;
                Ok(cancelled_output(budget.output_bytes))
            }
            EvaluationRace::TimedOut => {
                cancel_active_bounded(
                    &mut self.interpreter,
                    "timed_out",
                    "cell exceeded wall-clock budget",
                )
                .await?;
                Ok(timeout_output(budget.output_bytes))
            }
            EvaluationRace::TerminalPersistenceTimedOut => {
                let _ = cancel_active_bounded(
                    &mut self.interpreter,
                    "failed",
                    "terminal event persistence deadline exceeded",
                )
                .await;
                Err(tm_core::Error::Sandbox(
                    "terminal event persistence deadline exceeded".into(),
                ))
            }
        }
    }

    async fn eval_batch(
        &mut self,
        codes: &[String],
        budget: CellBudget,
    ) -> Result<Vec<EvalOutput>> {
        let usages = codes
            .iter()
            .map(|code| {
                binding_usage_bounded(
                    code,
                    self.limits.source_bytes,
                    self.limits.syntax_nodes,
                    self.limits.parse_depth,
                )
                .ok()
            })
            .collect::<Vec<_>>();
        let dependencies = batch_dependencies(&usages);
        let state_writing = usages
            .iter()
            .any(|usage| usage.as_ref().is_none_or(|usage| !usage.writes.is_empty()));
        if state_writing {
            // A fork emits `binding_committed` inside its own commit shield. The coordinator
            // cannot make that fork's private environment visible atomically if the outer batch
            // future is dropped, so state-writing (or unanalyzable) batches run directly on the
            // owning interpreter. Read/effect-only batches retain bounded parallel execution.
            let mut outputs = Vec::with_capacity(codes.len());
            let mut committed_by_cell = Vec::<Option<BTreeSet<String>>>::with_capacity(codes.len());
            for (index, code) in codes.iter().enumerate() {
                if let Some((dependency, names)) =
                    dependencies[index].iter().find_map(|(dependency, names)| {
                        committed_by_cell[*dependency]
                            .is_none()
                            .then_some((*dependency, names))
                    })
                {
                    let bindings = names.iter().cloned().collect::<Vec<_>>().join(", ");
                    let message = format!(
                        "BatchDependencyError: execute call {} requires binding(s) [{}] from failed execute call {}",
                        index + 1,
                        bindings,
                        dependency + 1
                    );
                    emit_dependency_failure_bounded(&mut self.interpreter, code, &message).await?;
                    let error = bounded_display(&message, budget.output_bytes);
                    outputs.push(EvalOutput {
                        error: Some(error),
                        ..EvalOutput::default()
                    });
                    committed_by_cell.push(None);
                    continue;
                }
                let cancellation = self.cancellation.clone();
                let (output, committed) =
                    eval_interpreter(&mut self.interpreter, code, budget, cancellation.as_deref())
                        .await?;
                outputs.push(output);
                committed_by_cell.push(committed);
            }
            return Ok(outputs);
        }

        let base = self.interpreter.clone();
        let mut pending = (0..codes.len()).collect::<BTreeSet<_>>();
        let mut results: Vec<Option<(Interpreter, EvalOutput, Option<BTreeSet<String>>)>> =
            vec![None; codes.len()];

        while !pending.is_empty() {
            let ready = pending
                .iter()
                .copied()
                .filter(|index| {
                    dependencies[*index]
                        .keys()
                        .all(|dependency| results[*dependency].is_some())
                })
                .collect::<Vec<_>>();
            debug_assert!(
                !ready.is_empty(),
                "forward-only batch graph must make progress"
            );

            let evaluations = ready.iter().map(|index| {
                let index = *index;
                let mut interpreter = base.fork_for_parallel(index as u64);
                let failed = dependencies[index].iter().find_map(|(dependency, names)| {
                    results[*dependency]
                        .as_ref()
                        .and_then(|(_, _, committed)| {
                            committed
                                .is_none()
                                .then_some((*dependency, names.iter().cloned().collect::<Vec<_>>()))
                        })
                });
                let successful_dependencies = dependencies[index]
                    .keys()
                    .filter_map(|dependency| {
                        results[*dependency].as_ref().and_then(
                            |(fork, _, committed)| {
                                committed
                                    .as_ref()
                                    .map(|names| (fork.clone(), names.clone()))
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                for (fork, committed) in successful_dependencies {
                    interpreter.merge_committed_from(&fork, &committed);
                }
                let cancellation = self.cancellation.clone();
                let code = &codes[index];
                async move {
                    let output = if let Some((dependency, names)) = failed {
                        let bindings = names.join(", ");
                        let message = format!(
                            "BatchDependencyError: execute call {} requires binding(s) [{}] from failed execute call {}",
                            index + 1,
                            bindings,
                            dependency + 1
                        );
                        match emit_dependency_failure_bounded(
                            &mut interpreter,
                            code,
                            &message,
                        )
                        .await
                        {
                            Ok(()) => Ok((
                                EvalOutput {
                                    error: Some(bounded_display(&message, budget.output_bytes)),
                                    ..EvalOutput::default()
                                },
                                None,
                            )),
                            Err(error) => Err(error),
                        }
                    } else {
                        eval_interpreter(
                            &mut interpreter,
                            code,
                            budget,
                            cancellation.as_deref(),
                        )
                        .await
                    };
                    (index, interpreter, output)
                }
            }).collect::<Vec<_>>();

            let mut evaluations =
                futures::stream::iter(evaluations).buffer_unordered(self.limits.parallelism.max(1));
            let mut wave_error = None;
            while let Some(evaluation) = evaluations.next().await {
                let (index, interpreter, evaluation) = evaluation;
                pending.remove(&index);
                match evaluation {
                    Ok((output, committed)) => {
                        results[index] = Some((interpreter, output, committed));
                    }
                    Err(error) if wave_error.is_none() => wave_error = Some(error),
                    Err(_) => {}
                }
            }
            if let Some(error) = wave_error {
                // Some siblings may already have durably emitted `binding_committed`. Merge every
                // successful fork in response order before surfacing a later sink/runtime error.
                for (fork, _, committed) in results.iter().flatten() {
                    if let Some(committed) = committed {
                        self.interpreter.merge_committed_from(fork, committed);
                    }
                }
                return Err(error);
            }
        }

        let mut outputs = Vec::with_capacity(results.len());
        for result in results.into_iter().flatten() {
            let (fork, output, committed) = result;
            if let Some(committed) = &committed {
                self.interpreter.merge_committed_from(&fork, committed);
            }
            outputs.push(output);
        }
        self.interpreter.finish_parallel_batch(codes.len() as u64);
        Ok(outputs)
    }

    async fn reset(&mut self) -> Result<()> {
        self.interpreter.reset();
        Ok(())
    }
}

fn batch_dependencies(
    usages: &[Option<crate::batch::BindingUsage>],
) -> Vec<BTreeMap<usize, BTreeSet<String>>> {
    let mut writers = BTreeMap::<String, usize>::new();
    let mut unknown_writers = Vec::new();
    let mut dependencies = Vec::with_capacity(usages.len());
    for (index, usage) in usages.iter().enumerate() {
        let mut cell = BTreeMap::<usize, BTreeSet<String>>::new();
        for writer in &unknown_writers {
            cell.entry(*writer)
                .or_default()
                .insert("<unknown bindings>".to_string());
        }
        if let Some(usage) = usage {
            for name in &usage.reads {
                if let Some(writer) = writers.get(name) {
                    cell.entry(*writer).or_default().insert(name.clone());
                }
            }
            for name in &usage.writes {
                writers.insert(name.clone(), index);
            }
        } else {
            unknown_writers.push(index);
        }
        dependencies.push(cell);
    }
    dependencies
}

async fn eval_interpreter(
    interpreter: &mut Interpreter,
    code: &str,
    budget: CellBudget,
    cancellation: Option<&dyn CancellationToken>,
) -> Result<(EvalOutput, Option<BTreeSet<String>>)> {
    if budget.wall_ms == 0 {
        emit_immediate_bounded(
            interpreter,
            code,
            "timed_out",
            "cell exceeded wall-clock budget",
        )
        .await?;
        return Ok((timeout_output(budget.output_bytes), None));
    }
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        emit_immediate_bounded(interpreter, code, "cancelled", "cell cancelled").await?;
        return Ok((cancelled_output(budget.output_bytes), None));
    }
    let result = race_evaluation(interpreter, code, budget, cancellation).await;
    match result {
        EvaluationRace::Completed(Ok(output)) => {
            let committed = output.committed.clone();
            Ok((
                EvalOutput {
                    stdout: output.stdout,
                    result: Some(output.value.to_json()),
                    error: None,
                },
                Some(committed),
            ))
        }
        EvaluationRace::Completed(Err(RuntimeError::Persistence(error))) => {
            interpreter.abandon_active_eval();
            Err(tm_core::Error::Sandbox(error))
        }
        EvaluationRace::Completed(Err(error)) => Ok((
            EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(bounded_display(&error, budget.output_bytes)),
            },
            None,
        )),
        EvaluationRace::Cancelled => {
            cancel_active_bounded(interpreter, "cancelled", "cell cancelled").await?;
            Ok((cancelled_output(budget.output_bytes), None))
        }
        EvaluationRace::TimedOut => {
            cancel_active_bounded(interpreter, "timed_out", "cell exceeded wall-clock budget")
                .await?;
            Ok((timeout_output(budget.output_bytes), None))
        }
        EvaluationRace::TerminalPersistenceTimedOut => {
            let _ = cancel_active_bounded(
                interpreter,
                "failed",
                "terminal event persistence deadline exceeded",
            )
            .await;
            Err(tm_core::Error::Sandbox(
                "terminal event persistence deadline exceeded".into(),
            ))
        }
    }
}

fn cancelled_output(output_bytes: usize) -> EvalOutput {
    EvalOutput {
        stdout: String::new(),
        result: None,
        error: Some(bounded_display(
            &"CancellationError: cell cancelled",
            output_bytes,
        )),
    }
}

fn timeout_output(output_bytes: usize) -> EvalOutput {
    EvalOutput {
        stdout: String::new(),
        result: None,
        error: Some(bounded_display(
            &"TimeoutError: cell exceeded wall-clock budget",
            output_bytes,
        )),
    }
}
