//! Host capability, resource, and approval foundations.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde_json::Value;
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
    #[error("host call error: {0}")]
    HostCall(String),
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

    pub fn permits(&self, name: &str) -> bool {
        self.allowed.contains(name)
    }
}

#[derive(Debug, Clone)]
pub struct InvocationCtx {
    pub grants: CapabilityGrants,
}

impl InvocationCtx {
    pub fn new(grants: CapabilityGrants) -> Self {
        Self { grants }
    }
}

#[async_trait]
pub trait HostFn: Send + Sync {
    fn name(&self) -> &str;
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
        handler.read(uri, selector, ctx).await
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

    struct EchoFn;

    #[async_trait]
    impl HostFn for EchoFn {
        fn name(&self) -> &str {
            "echo"
        }

        async fn call(&self, args: Value, _ctx: &InvocationCtx) -> Result<Value> {
            Ok(args)
        }
    }

    #[tokio::test]
    async fn unknown_capability_fails_closed() {
        let mut registry = HostRegistry::new();
        registry.register(Arc::new(EchoFn));
        let ctx = InvocationCtx::new(CapabilityGrants::default());
        let err = registry
            .invoke("echo", Value::String("x".into()), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, HostError::CapabilityDenied("echo".into()));
    }

    #[tokio::test]
    async fn unknown_scheme_fails_closed() {
        let registry = ResourceRegistry::new();
        let ctx = InvocationCtx::new(CapabilityGrants::default());
        let err = registry.read("memory://x", None, &ctx).await.unwrap_err();
        assert!(matches!(err, HostError::UnknownScheme { .. }));
    }

    #[tokio::test]
    async fn approval_default_denies_on_timeout() {
        let policy = DefaultDenyApprovalPolicy;
        let err = policy
            .request("write-prod", Duration::from_millis(1))
            .await
            .unwrap_err();
        assert_eq!(err, HostError::ApprovalTimeout("write-prod".into()));
    }

    #[tokio::test]
    async fn artifact_handler_resolves_through_registry() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(dir.path(), "default").unwrap();
        let artifact = store.put_text("hello", None, "text/plain").unwrap();
        let mut registry = ResourceRegistry::new();
        registry.register(Arc::new(ArtifactResourceHandler::new(store)));
        let ctx = InvocationCtx::new(CapabilityGrants::default().allow("resources.read:artifact"));
        let content = registry.read(&artifact.uri, None, &ctx).await.unwrap();
        assert_eq!(content.content, "hello");
    }
}
