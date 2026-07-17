use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::future::join_all;
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;
use tm_core::{CancellationToken, CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};
use tm_drive::SharedDriveStore;
use tm_host::{
    ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants, DefaultDenyApprovalPolicy, GrantDoc,
    HostError, HostEventSink, HostFn, HostRegistry, InvocationCtx, LinkedFolders,
    NoopHostEventSink, ResourceRegistry, ToolDocs, register_p0_linked_folder_functions,
};

use crate::{Interpreter, RuntimeLimits, batch::binding_usage, catalog_from_registry};

pub const CORE_TM_CAPABILITIES: &[&str] = &["http.get", "resources.read:artifact"];

pub fn core_tm_grants() -> CapabilityGrants {
    CapabilityGrants::default().allow_many(CORE_TM_CAPABILITIES.iter().copied())
}

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
            grants: core_tm_grants(),
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
        let artifacts = ArtifactStore::open(&self.options.artifact_root, &self.options.session_id)
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
                linked_folders,
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
        let mut resource_schemes = resources
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
        invocation.grants = invocation
            .grants
            .clone()
            .allow_many(["artifacts.put", "artifacts.list"]);
        if invocation.grants.permits("resources.read:artifact") {
            invocation.grants = invocation
                .grants
                .clone()
                .allow_many(["artifacts.get", "artifacts.slice"]);
            if !resource_schemes.iter().any(|scheme| scheme == "artifact") {
                resource_schemes.push("artifact".into());
            }
        }
        let catalog_registry = Arc::new(registry.clone());
        for operation in [CatalogOperation::Search, CatalogOperation::Docs] {
            registry.register(Arc::new(CatalogFn::new(
                operation,
                Arc::clone(&catalog_registry),
            )));
        }
        invocation.grants = invocation
            .grants
            .clone()
            .allow_many(["tools.search", "tools.docs"]);
        let registry = Arc::new(registry);
        let catalog = catalog_from_registry(&registry, &invocation, resource_schemes);
        Ok(Box::new(TmSession {
            interpreter: Interpreter::new(
                catalog,
                registry,
                invocation,
                self.options.limits.clone(),
            ),
            cancellation: self.options.cancellation.clone(),
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
            ArtifactOperation::Get => ("artifacts.get", "Read a session artifact", false),
            ArtifactOperation::Slice => {
                ("artifacts.slice", "Read a selected artifact slice", false)
            }
            ArtifactOperation::List => ("artifacts.list", "List session artifacts", false),
        };
        Self {
            operation,
            store,
            docs: ToolDocs {
                name: name.into(),
                namespace: "artifacts".into(),
                summary: summary.into(),
                description: None,
                signature: format!("{name}(args)"),
                args_schema: json!({}),
                result_schema: None,
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "artifact".into(),
                    description: match operation {
                        ArtifactOperation::Get | ArtifactOperation::Slice => {
                            "Reads require resources.read:artifact"
                        }
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
                serde_json::to_value(
                    self.store
                        .put_text(content, title, &mime)
                        .map_err(|error| HostError::HostCall(error.to_string()))?,
                )
                .map_err(|error| HostError::HostCall(error.to_string()))
            }
            ArtifactOperation::Get | ArtifactOperation::Slice => {
                if !ctx.grants.permits("resources.read:artifact") {
                    return Err(HostError::CapabilityDenied(
                        "resources.read:artifact".into(),
                    ));
                }
                let (uri, selector) = artifact_read_args(&args, self.operation)?;
                serde_json::to_value(
                    self.store
                        .read(&uri, selector.as_deref())
                        .map_err(|error| HostError::NotFound(error.to_string()))?,
                )
                .map_err(|error| HostError::HostCall(error.to_string()))
            }
            ArtifactOperation::List => serde_json::to_value(self.store.list())
                .map_err(|error| HostError::HostCall(error.to_string())),
        }
    }
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
                sensitive: false,
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
}

#[async_trait(?Send)]
impl Session for TmSession {
    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput> {
        if budget.wall_ms == 0 {
            return Ok(EvalOutput {
                error: Some("TimeoutError: cell exceeded wall-clock budget".into()),
                ..EvalOutput::default()
            });
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            return Ok(cancelled_output());
        }
        let evaluation = tokio::time::timeout(
            std::time::Duration::from_millis(budget.wall_ms),
            self.interpreter.eval(code, budget.output_bytes),
        );
        let result = if let Some(token) = &self.cancellation {
            tokio::select! {
                _ = token.cancelled() => return Ok(cancelled_output()),
                result = evaluation => result,
            }
        } else {
            evaluation.await
        };
        match result {
            Ok(Ok(output)) => Ok(EvalOutput {
                stdout: output.stdout,
                result: Some(output.value.to_json()),
                error: None,
            }),
            Ok(Err(error)) => Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(error.to_string()),
            }),
            Err(_) => Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some("TimeoutError: cell exceeded wall-clock budget".into()),
            }),
        }
    }

    async fn eval_batch(
        &mut self,
        codes: &[String],
        budget: CellBudget,
    ) -> Result<Vec<EvalOutput>> {
        let base = self.interpreter.clone();
        let usages = codes
            .iter()
            .map(|code| binding_usage(code).ok())
            .collect::<Vec<_>>();
        let dependencies = batch_dependencies(&usages);
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
                        .and_then(|(_, _, committed)| committed.is_none().then_some((*dependency, names)))
                });
                let successful_dependencies = dependencies[index]
                    .keys()
                    .filter_map(|dependency| {
                        results[*dependency].as_ref().and_then(
                            |(fork, _, committed)| committed.as_ref().map(|names| (fork, names)),
                        )
                    })
                    .collect::<Vec<_>>();
                for (fork, committed) in successful_dependencies {
                    interpreter.merge_committed_from(fork, committed);
                }
                let cancellation = self.cancellation.clone();
                let code = &codes[index];
                async move {
                    let output = if let Some((dependency, names)) = failed {
                        let bindings = names.iter().cloned().collect::<Vec<_>>().join(", ");
                        let message = format!(
                            "BatchDependencyError: execute call {} requires binding(s) [{}] from failed execute call {}",
                            index + 1,
                            bindings,
                            dependency + 1
                        );
                        let error = interpreter
                            .emit_dependency_failure(code, &message)
                            .await
                            .err()
                            .map_or(message, |error| error.to_string());
                        (
                            EvalOutput {
                                error: Some(error),
                                ..EvalOutput::default()
                            },
                            None,
                        )
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
            });

            for (index, interpreter, (output, committed)) in join_all(evaluations).await {
                pending.remove(&index);
                results[index] = Some((interpreter, output, committed));
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
) -> (EvalOutput, Option<BTreeSet<String>>) {
    if budget.wall_ms == 0 {
        return (
            EvalOutput {
                error: Some("TimeoutError: cell exceeded wall-clock budget".into()),
                ..EvalOutput::default()
            },
            None,
        );
    }
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return (cancelled_output(), None);
    }
    let evaluation = tokio::time::timeout(
        std::time::Duration::from_millis(budget.wall_ms),
        interpreter.eval(code, budget.output_bytes),
    );
    let result = if let Some(token) = cancellation {
        tokio::select! {
            _ = token.cancelled() => return (cancelled_output(), None),
            result = evaluation => result,
        }
    } else {
        evaluation.await
    };
    match result {
        Ok(Ok(output)) => {
            let committed = output.committed.clone();
            (
                EvalOutput {
                    stdout: output.stdout,
                    result: Some(output.value.to_json()),
                    error: None,
                },
                Some(committed),
            )
        }
        Ok(Err(error)) => (
            EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(error.to_string()),
            },
            None,
        ),
        Err(_) => (
            EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some("TimeoutError: cell exceeded wall-clock budget".into()),
            },
            None,
        ),
    }
}

fn cancelled_output() -> EvalOutput {
    EvalOutput {
        stdout: String::new(),
        result: None,
        error: Some("CancellationError: cell cancelled".into()),
    }
}
