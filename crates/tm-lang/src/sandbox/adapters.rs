use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;
use tm_host::{
    GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry, ToolDocs,
};

#[derive(Debug, Clone)]
pub(super) struct HttpGetFn {
    responses: BTreeMap<String, String>,
    docs: ToolDocs,
}

impl HttpGetFn {
    pub(super) fn new(responses: BTreeMap<String, String>) -> Self {
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
pub(super) enum ArtifactOperation {
    Put,
    Get,
    Slice,
    List,
}

pub(super) struct ArtifactFn {
    operation: ArtifactOperation,
    store: ArtifactStore,
    docs: ToolDocs,
}

impl ArtifactFn {
    pub(super) fn new(operation: ArtifactOperation, store: ArtifactStore) -> Self {
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
pub(super) enum CatalogOperation {
    Search,
    Docs,
    Call,
}

pub(super) struct CatalogFn {
    operation: CatalogOperation,
    registry: Arc<HostRegistry>,
    docs: ToolDocs,
}

impl CatalogFn {
    pub(super) fn new(operation: CatalogOperation, registry: Arc<HostRegistry>) -> Self {
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
            CatalogOperation::Call => (
                "tools.call",
                "Invoke one granted tm effect by its catalog name",
                "tools.call({name, args})",
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
                // A late-bound target may carry private arguments or results even though catalog
                // search/docs do not. Redact the generic call envelope at the runtime-event
                // boundary; the target HostFn still performs its own exact grant and approval
                // checks through HostRegistry::invoke.
                sensitive: matches!(operation, CatalogOperation::Call),
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
            CatalogOperation::Call => {
                let fields = args.as_object().ok_or_else(|| {
                    HostError::InvalidArgs("tools.call requires {name: String, args: Json}".into())
                })?;
                if fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "name" | "args"))
                {
                    return Err(HostError::InvalidArgs(
                        "tools.call accepts only name and args".into(),
                    ));
                }
                let name = fields.get("name").and_then(Value::as_str).ok_or_else(|| {
                    HostError::InvalidArgs("tools.call requires a string name".into())
                })?;
                if name == "tools.call" {
                    return Err(HostError::InvalidArgs(
                        "tools.call cannot recursively invoke itself".into(),
                    ));
                }
                let target_args = fields.get("args").cloned().unwrap_or(Value::Null);
                self.registry.invoke(name, target_args, ctx).await
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ResourceOperation {
    Read,
    Preview,
    List,
}

pub(super) struct ResourceFn {
    operation: ResourceOperation,
    resources: ResourceRegistry,
    docs: ToolDocs,
}

impl ResourceFn {
    pub(super) fn new(operation: ResourceOperation, resources: ResourceRegistry) -> Self {
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
