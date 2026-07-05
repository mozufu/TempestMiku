use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tm_artifacts::{ArtifactStore, ResourceContent};
use url::Url;

pub type Result<T, E = HostError> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HostError {
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    #[error("approval denied: {0}")]
    ApprovalDenied(String),
    #[error("approval timed out: {0}")]
    ApprovalTimeout(String),
    #[error("unknown resource scheme: {scheme}; registered: {registered:?}")]
    UnknownScheme {
        scheme: String,
        registered: Vec<String>,
    },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("output truncated: {0}")]
    OutputTruncated(String),
    #[error("host call error: {0}")]
    HostCall(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostErrorPayload {
    pub name: String,
    pub message: String,
    pub capability: Option<String>,
    pub path: Option<String>,
    pub uri: Option<String>,
    pub retryable: bool,
    pub details: Value,
}

impl HostError {
    pub fn sdk_name(&self) -> &'static str {
        match self {
            Self::CapabilityDenied(_) | Self::UnknownScheme { .. } => "CapabilityDeniedError",
            Self::ApprovalDenied(_) => "ApprovalDeniedError",
            Self::ApprovalTimeout(_) => "ApprovalTimeoutError",
            Self::NotFound(_) => "NotFoundError",
            Self::InvalidArgs(_) => "InvalidArgsError",
            Self::InvalidPath(_) => "InvalidPathError",
            Self::NotImplemented(_) => "NotImplementedError",
            Self::QuotaExceeded(_) => "QuotaExceededError",
            Self::Timeout(_) => "TimeoutError",
            Self::OutputTruncated(_) => "OutputTruncatedError",
            Self::HostCall(_) => "HostCallError",
        }
    }

    pub fn to_payload(&self) -> HostErrorPayload {
        let (capability, path, uri, retryable, details) = match self {
            Self::CapabilityDenied(capability) => (
                Some(capability.clone()),
                None,
                None,
                false,
                json!({ "capability": capability }),
            ),
            Self::ApprovalDenied(action) => (None, None, None, false, json!({ "action": action })),
            Self::ApprovalTimeout(action) => (None, None, None, true, json!({ "action": action })),
            Self::UnknownScheme { scheme, registered } => (
                None,
                None,
                Some(format!("{scheme}://")),
                false,
                json!({ "scheme": scheme, "registered": registered }),
            ),
            Self::NotFound(target) => (None, None, None, false, json!({ "target": target })),
            Self::InvalidArgs(message) => (None, None, None, false, json!({ "reason": message })),
            Self::InvalidPath(path) => (
                None,
                Some(path.clone()),
                None,
                false,
                json!({ "path": path }),
            ),
            Self::NotImplemented(feature) => {
                (None, None, None, false, json!({ "feature": feature }))
            }
            Self::QuotaExceeded(quota) => (None, None, None, true, json!({ "quota": quota })),
            Self::Timeout(operation) => (None, None, None, true, json!({ "operation": operation })),
            Self::OutputTruncated(target) => (None, None, None, false, json!({ "target": target })),
            Self::HostCall(message) => (None, None, None, false, json!({ "reason": message })),
        };
        HostErrorPayload {
            name: self.sdk_name().to_string(),
            message: self.to_string(),
            capability,
            path,
            uri,
            retryable,
            details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSummary {
    pub name: String,
    pub namespace: String,
    pub summary: String,
    pub sensitive: bool,
    pub granted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDocs {
    pub name: String,
    pub namespace: String,
    pub summary: String,
    pub description: Option<String>,
    pub signature: String,
    pub args_schema: Value,
    pub result_schema: Option<Value>,
    pub examples: Vec<ToolExample>,
    pub errors: Vec<ToolErrorDoc>,
    pub grants: Vec<GrantDoc>,
    pub sensitive: bool,
    pub approval: String,
    pub since: String,
    pub stability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExample {
    pub title: Option<String>,
    pub code: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolErrorDoc {
    pub name: String,
    pub when: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantDoc {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityGrants {
    allowed: BTreeSet<String>,
}

impl CapabilityGrants {
    pub fn allow(mut self, name: impl Into<String>) -> Self {
        self.allowed.insert(name.into());
        self
    }

    pub fn allow_many(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for name in names {
            self.allowed.insert(name.into());
        }
        self
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(String::as_str)
    }

    pub fn permits(&self, name: &str) -> bool {
        self.allowed.iter().any(|granted| {
            if let Some(prefix) = granted.strip_suffix(".*") {
                name == prefix || name.starts_with(&format!("{prefix}."))
            } else {
                granted == name
            }
        })
    }
}

#[derive(Clone)]
pub struct InvocationCtx {
    pub grants: CapabilityGrants,
    pub approvals: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
    /// Session UUID string; empty when no session context is available.
    pub session_id: String,
    /// Current actor id, when the host call originates inside a sub-agent sandbox.
    ///
    /// Top-level orchestrator sessions leave this unset and are treated as `Root`
    /// by the agents mailbox layer.
    pub actor_id: Option<String>,
}

impl std::fmt::Debug for InvocationCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationCtx")
            .field("grants", &self.grants)
            .field("approval_timeout", &self.approval_timeout)
            .field("session_id", &self.session_id)
            .field("actor_id", &self.actor_id)
            .finish_non_exhaustive()
    }
}

impl InvocationCtx {
    pub fn new(grants: CapabilityGrants) -> Self {
        Self::with_approvals(
            grants,
            Arc::new(DefaultDenyApprovalPolicy),
            Duration::from_secs(60),
        )
    }

    pub fn with_approvals(
        grants: CapabilityGrants,
        approvals: Arc<dyn ApprovalPolicy>,
        approval_timeout: Duration,
    ) -> Self {
        Self {
            grants,
            approvals,
            approval_timeout,
            session_id: String::new(),
            actor_id: None,
        }
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    pub fn with_actor_id(mut self, actor_id: Option<impl Into<String>>) -> Self {
        self.actor_id = actor_id.map(Into::into);
        self
    }

    pub async fn require_approval(&self, action: &str) -> Result<()> {
        match self
            .approvals
            .request(action, self.approval_timeout)
            .await?
        {
            ApprovalDecision::Approved => Ok(()),
            ApprovalDecision::Denied => Err(HostError::ApprovalDenied(action.to_string())),
        }
    }
}

#[async_trait]
pub trait HostFn: Send + Sync {
    fn docs(&self) -> &ToolDocs;

    fn name(&self) -> &str {
        &self.docs().name
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value>;
}

#[derive(Default, Clone)]
pub struct HostRegistry {
    functions: BTreeMap<String, Arc<dyn HostFn>>,
}

impl HostRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, function: Arc<dyn HostFn>) {
        self.functions.insert(function.name().to_string(), function);
    }

    pub fn search(
        &self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
        ctx: &InvocationCtx,
    ) -> Vec<ToolSummary> {
        let needle = query.to_lowercase();
        let limit = limit.max(1);
        self.functions
            .values()
            .filter_map(|function| {
                let docs = function.docs();
                if let Some(namespace) = namespace
                    && docs.namespace != namespace
                {
                    return None;
                }
                let haystack =
                    format!("{} {} {}", docs.name, docs.namespace, docs.summary).to_lowercase();
                (needle.is_empty() || haystack.contains(&needle)).then(|| ToolSummary {
                    name: docs.name.clone(),
                    namespace: docs.namespace.clone(),
                    summary: docs.summary.clone(),
                    sensitive: docs.sensitive,
                    granted: ctx.grants.permits(&docs.name),
                })
            })
            .take(limit)
            .collect()
    }

    pub fn docs(&self, name: &str, _ctx: &InvocationCtx) -> Result<ToolDocs> {
        self.functions
            .get(name)
            .map(|function| function.docs().clone())
            .ok_or_else(|| HostError::NotFound(format!("tool {name}")))
    }

    pub async fn invoke(&self, name: &str, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        if !ctx.grants.permits(name) {
            return Err(HostError::CapabilityDenied(name.to_string()));
        }
        let function = self
            .functions
            .get(name)
            .ok_or_else(|| HostError::CapabilityDenied(name.to_string()))?;
        function.call(args, ctx).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    async fn request(&self, action: &str, timeout: Duration) -> Result<ApprovalDecision>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultDenyApprovalPolicy;

#[async_trait]
impl ApprovalPolicy for DefaultDenyApprovalPolicy {
    async fn request(&self, action: &str, _timeout: Duration) -> Result<ApprovalDecision> {
        Err(HostError::ApprovalTimeout(action.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceEntry {
    pub uri: String,
    pub name: String,
    pub kind: String,
    pub title: Option<String>,
    pub size_bytes: Option<usize>,
    pub modified_at: Option<String>,
}

#[async_trait]
pub trait ResourceHandler: Send + Sync {
    fn scheme(&self) -> &str;
    fn capability(&self) -> &str;
    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent>;

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        let mut content = self.read(uri, None, ctx).await?;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        Err(HostError::NotFound(format!(
            "resource list unsupported for {} {}",
            self.scheme(),
            uri.unwrap_or("")
        )))
    }
}

#[derive(Default, Clone)]
pub struct ResourceRegistry {
    handlers: BTreeMap<String, Arc<dyn ResourceHandler>>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn ResourceHandler>) {
        self.handlers.insert(handler.scheme().to_string(), handler);
    }

    pub fn schemes(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    pub async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        let handler = self.handler_for(uri, ctx)?;
        handler.read(uri, selector, ctx).await
    }

    pub async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> Result<ResourceContent> {
        let handler = self.handler_for(uri, ctx)?;
        handler.preview(uri, ctx).await
    }

    pub async fn list(&self, uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let Some(uri) = uri.filter(|uri| !uri.is_empty()) else {
            return Ok(self
                .handlers
                .keys()
                .map(|scheme| ResourceEntry {
                    uri: format!("{scheme}://"),
                    name: scheme.clone(),
                    kind: "scheme".to_string(),
                    title: None,
                    size_bytes: None,
                    modified_at: None,
                })
                .collect());
        };
        let handler = self.handler_for(uri, ctx)?;
        handler.list(Some(uri), ctx).await
    }

    fn handler_for(&self, uri: &str, ctx: &InvocationCtx) -> Result<Arc<dyn ResourceHandler>> {
        let scheme = parse_scheme(uri)?;
        let handler = self
            .handlers
            .get(&scheme)
            .ok_or_else(|| HostError::UnknownScheme {
                scheme: scheme.clone(),
                registered: self.schemes(),
            })?;
        if !ctx.grants.permits(handler.capability()) {
            return Err(HostError::CapabilityDenied(
                handler.capability().to_string(),
            ));
        }
        Ok(Arc::clone(handler))
    }
}

pub struct ArtifactResourceHandler {
    store: ArtifactStore,
}

impl ArtifactResourceHandler {
    pub fn new(store: ArtifactStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ResourceHandler for ArtifactResourceHandler {
    fn scheme(&self) -> &str {
        "artifact"
    }

    fn capability(&self) -> &str {
        "resources.read:artifact"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        self.store
            .read(uri, selector)
            .map_err(|err| HostError::NotFound(err.to_string()))
    }

    async fn list(&self, _uri: Option<&str>, _ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        Ok(self
            .store
            .list()
            .into_iter()
            .map(|artifact| ResourceEntry {
                uri: artifact.uri,
                name: artifact.id,
                kind: artifact.kind,
                title: artifact.title,
                size_bytes: Some(artifact.size_bytes),
                modified_at: None,
            })
            .collect())
    }
}

fn parse_scheme(uri: &str) -> Result<String> {
    if let Ok(url) = Url::parse(uri) {
        return Ok(url.scheme().to_string());
    }
    uri.split_once("://")
        .map(|(scheme, _)| scheme.to_string())
        .ok_or_else(|| HostError::InvalidArgs(format!("missing URI scheme in {uri}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_grants_exact_match() {
        let g = CapabilityGrants::default().allow("agents.run");
        assert!(g.permits("agents.run"));
        assert!(!g.permits("agents.spawn"));
        assert!(!g.permits("agents"));
    }

    #[test]
    fn capability_grants_glob_match() {
        let g = CapabilityGrants::default().allow("agents.*");
        assert!(g.permits("agents.run"));
        assert!(g.permits("agents.spawn"));
        assert!(g.permits("agents.parallel"));
        assert!(g.permits("agents.msg"));
        assert!(g.permits("agents.send"));
        assert!(g.permits("agents.wait"));
        assert!(g.permits("agents.inbox"));
        assert!(g.permits("agents.list"));
        assert!(!g.permits("other.run"));
        assert!(
            !g.permits("agents_run"),
            "underscore variant must not match"
        );
    }

    #[test]
    fn capability_grants_names_includes_glob() {
        let g = CapabilityGrants::default().allow("agents.*");
        assert!(g.names().any(|n| n.starts_with("agents.")));
    }
}
