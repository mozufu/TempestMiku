use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{HostError, Result};

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
    pub events: Arc<dyn HostEventSink>,
    /// Session UUID string; empty when no session context is available.
    pub session_id: String,
    /// Current actor id, when the host call originates inside a sub-agent sandbox.
    ///
    /// Top-level orchestrator sessions leave this unset and are treated as `Root`
    /// by the agents mailbox layer.
    pub actor_id: Option<String>,
    /// Server-authoritative session scope used by project-bound capability families.
    pub session_scope: Option<String>,
    /// Server-authoritative owner and memory scope for resource calls.
    ///
    /// Product handlers must compare requested subjects/scopes against this
    /// value instead of trusting model- or client-supplied arguments.
    pub memory_authority: Option<MemoryAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAuthority {
    pub subject: String,
    pub scope: String,
}

impl std::fmt::Debug for InvocationCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationCtx")
            .field("grants", &self.grants)
            .field("approval_timeout", &self.approval_timeout)
            .field("session_id", &self.session_id)
            .field("actor_id", &self.actor_id)
            .field("session_scope", &self.session_scope)
            .field("memory_authority", &self.memory_authority)
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
            events: Arc::new(NoopHostEventSink),
            session_id: String::new(),
            actor_id: None,
            session_scope: None,
            memory_authority: None,
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

    pub fn with_session_scope(mut self, scope: impl Into<String>) -> Self {
        self.session_scope = Some(scope.into());
        self
    }

    pub fn require_linked_alias(&self, alias: &str) -> Result<()> {
        let Some(scope) = self.session_scope.as_deref() else {
            if self.session_id.is_empty() || self.session_id == "default" {
                return Ok(());
            }
            return Err(HostError::CapabilityDenied(
                "linked resources require server-authoritative project scope".to_string(),
            ));
        };
        let Some(project) = scope.strip_prefix("project:") else {
            return Err(HostError::CapabilityDenied(format!(
                "linked resources are unavailable from non-project session scope {scope}"
            )));
        };
        if project == alias {
            Ok(())
        } else {
            Err(HostError::CapabilityDenied(format!(
                "linked alias {alias} is outside authorized scope {scope}"
            )))
        }
    }

    pub fn with_memory_authority(
        mut self,
        subject: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        self.memory_authority = Some(MemoryAuthority {
            subject: subject.into(),
            scope: scope.into(),
        });
        self
    }

    pub fn with_event_sink(mut self, events: Arc<dyn HostEventSink>) -> Self {
        self.events = events;
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

    pub async fn emit_event(&self, event_type: &str, payload_json: Value) -> Result<()> {
        self.events.emit(event_type, payload_json).await
    }
}

#[async_trait]
pub trait HostEventSink: Send + Sync {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<()>;
}

pub struct NoopHostEventSink;

#[async_trait]
impl HostEventSink for NoopHostEventSink {
    async fn emit(&self, _event_type: &str, _payload_json: Value) -> Result<()> {
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_authority_is_explicit_and_server_supplied() {
        let ctx = InvocationCtx::new(CapabilityGrants::default())
            .with_memory_authority("brian", "project:tempestmiku");

        assert_eq!(
            ctx.memory_authority,
            Some(MemoryAuthority {
                subject: "brian".to_string(),
                scope: "project:tempestmiku".to_string(),
            })
        );
    }
}
