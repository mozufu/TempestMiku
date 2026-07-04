use std::sync::Arc;

use async_trait::async_trait;
use tm_artifacts::ResourceContent;
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler, Result};

use crate::mailbox::MailboxRegistry;

/// Capability required to read `agent://` resources (§23.7).
pub const CAP_AGENT_READ: &str = "resources.read:agent";

/// Capability required to read `history://` resources (§23.7).
pub const CAP_HISTORY_READ: &str = "resources.read:history";

/// Serves `agent://<id>` — live actor record reads and listing (§23.7).
///
/// Full transcript/artifact content is deferred to P3.3; this serves the record JSON.
pub struct AgentResourceHandler {
    roster: Arc<MailboxRegistry>,
}

impl AgentResourceHandler {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self { roster }
    }
}

#[async_trait]
impl ResourceHandler for AgentResourceHandler {
    fn scheme(&self) -> &str {
        "agent"
    }

    fn capability(&self) -> &str {
        CAP_AGENT_READ
    }

    async fn read(
        &self,
        uri: &str,
        _selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        // uri = "agent://<id>"
        let id_str = uri
            .strip_prefix("agent://")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| HostError::InvalidArgs(format!("invalid agent:// URI: {uri}")))?;

        let actor_id = crate::actor::ActorId::new(id_str)
            .map_err(|e| HostError::NotFound(format!("invalid actor id {id_str:?}: {e}")))?;

        let record = self
            .roster
            .get(&actor_id)
            .await
            .ok_or_else(|| HostError::NotFound(format!("actor {id_str} not found")))?;

        let content = serde_json::to_string_pretty(&record)
            .map_err(|e| HostError::HostCall(format!("serialize actor record: {e}")))?;

        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: "actor".to_string(),
            mime: "application/json".to_string(),
            title: Some(format!("Actor {id_str}")),
            size_bytes: content.len(),
            selector: None,
            has_more: false,
            content,
            preview: String::new(),
        })
    }

    async fn list(
        &self,
        _uri: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<Vec<ResourceEntry>> {
        let records = self.roster.list().await;
        Ok(records
            .into_iter()
            .map(|r| ResourceEntry {
                uri: format!("agent://{}", r.id),
                name: r.id.to_string(),
                kind: "actor".to_string(),
                title: r.mode,
                size_bytes: None,
                modified_at: r.completed_at.map(|t| t.to_rfc3339()),
            })
            .collect())
    }
}

/// Serves `history://<id>` — read-only child transcript previews (§23.7).
///
/// Transcript storage is deferred to P3.3; returns a placeholder until transcripts are persisted.
pub struct HistoryResourceHandler {
    roster: Arc<MailboxRegistry>,
}

impl HistoryResourceHandler {
    pub fn new(roster: Arc<MailboxRegistry>) -> Self {
        Self { roster }
    }
}

#[async_trait]
impl ResourceHandler for HistoryResourceHandler {
    fn scheme(&self) -> &str {
        "history"
    }

    fn capability(&self) -> &str {
        CAP_HISTORY_READ
    }

    async fn read(
        &self,
        uri: &str,
        _selector: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<ResourceContent> {
        let id_str = uri
            .strip_prefix("history://")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| HostError::InvalidArgs(format!("invalid history:// URI: {uri}")))?;

        let actor_id = crate::actor::ActorId::new(id_str)
            .map_err(|e| HostError::NotFound(format!("invalid actor id {id_str:?}: {e}")))?;

        // Verify the actor exists.
        let record = self
            .roster
            .get(&actor_id)
            .await
            .ok_or_else(|| HostError::NotFound(format!("actor {id_str} not found")))?;

        // Transcript storage deferred to P3.3. Return a bounded placeholder.
        let content = format!(
            "Actor {} ({:?}) — full transcript storage deferred to P3.3.",
            record.id,
            record.status,
        );

        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: "history".to_string(),
            mime: "text/plain".to_string(),
            title: Some(format!("History {id_str}")),
            size_bytes: content.len(),
            selector: None,
            has_more: false,
            preview: String::new(),
            content,
        })
    }
}
