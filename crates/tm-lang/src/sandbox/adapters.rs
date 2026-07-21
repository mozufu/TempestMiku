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
pub(super) struct HttpRequestFixtureFn {
    responses: BTreeMap<String, String>,
    docs: ToolDocs,
}

impl HttpRequestFixtureFn {
    pub(super) fn new(responses: BTreeMap<String, String>) -> Self {
        Self {
            responses,
            docs: ToolDocs {
                name: "http.request".into(),
                namespace: "http".into(),
                summary: "Fetch a deterministic allowlisted HTTP response".into(),
                description: Some(
                    "Default-deny deterministic fixture access. This is not ambient network egress; production egress remains owned by P9. The fixture supports GET without a body only."
                        .into(),
                ),
                signature: "http.request({ method, url }) -> String".into(),
                args_schema: json!({
                    "type": "object",
                    "required": ["method", "url"],
                    "additionalProperties": false,
                    "properties": {
                        "method": { "type": "string" },
                        "url": { "type": "string", "format": "uri" }
                    }
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
impl HostFn for HttpRequestFixtureFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let method = args.get("method").and_then(Value::as_str).ok_or_else(|| {
            HostError::InvalidArgs("http.request requires a string method".into())
        })?;
        if !method.eq_ignore_ascii_case("GET") || args.get("body").is_some() {
            return Err(HostError::InvalidArgs(
                "http.request fixture supports GET without a body only".into(),
            ));
        }
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| HostError::InvalidArgs("http.request requires a string url".into()))?;
        self.responses
            .get(url)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| HostError::CapabilityDenied("http.request".into()))
    }
}

pub(super) struct ArtifactFn {
    store: ArtifactStore,
    docs: ToolDocs,
}

impl ArtifactFn {
    pub(super) fn new(store: ArtifactStore) -> Self {
        Self {
            store,
            docs: ToolDocs {
                name: "artifacts.put".into(),
                namespace: "artifacts".into(),
                summary: "Store a session artifact".into(),
                description: None,
                signature: "artifacts.put(args)".into(),
                args_schema: json!({}),
                result_schema: None,
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "artifact".into(),
                    description: "Session-local artifact operation".into(),
                }],
                sensitive: true,
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

    async fn call(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
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
        let artifact = tokio::task::spawn_blocking(move || store.put_text(content, title, &mime))
            .await
            .map_err(|error| HostError::HostCall(format!("artifact write task failed: {error}")))?
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        serde_json::to_value(artifact).map_err(|error| HostError::HostCall(error.to_string()))
    }
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
