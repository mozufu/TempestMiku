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
        selector: Option<&str>,
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

        let full = self
            .roster
            .get_transcript(&actor_id)
            .await
            .unwrap_or_else(|| {
                format!(
                    "Actor {} ({:?}) — no transcript captured yet.",
                    record.id, record.status,
                )
            });

        let lines: Vec<&str> = full.lines().collect();
        let total_lines = lines.len();

        // Parse optional "start-end" selector for bounded mobile previews.
        let (start, end) = parse_selector(selector, total_lines);
        let selected: String = lines[start..end].join("\n");
        let has_more = end < total_lines;

        // Preview: first 1 KB of the full transcript.
        const PREVIEW_BYTES: usize = 1024;
        let preview: String = full.chars().take(PREVIEW_BYTES).collect();

        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: "history".to_string(),
            mime: "text/plain".to_string(),
            title: record.mode.as_deref().map(|m| format!("{m} transcript")),
            size_bytes: full.len(),
            selector: selector.map(str::to_string),
            has_more,
            preview,
            content: selected,
        })
    }

    async fn list(
        &self,
        _uri: Option<&str>,
        _ctx: &InvocationCtx,
    ) -> Result<Vec<ResourceEntry>> {
        let entries = self
            .roster
            .list()
            .await
            .into_iter()
            .filter(|rec| rec.history_uri.is_some())
            .map(|rec| ResourceEntry {
                uri: format!("history://{}", rec.id),
                name: rec.id.to_string(),
                kind: "history".to_string(),
                title: rec.mode,
                size_bytes: None,
                modified_at: rec.completed_at.map(|t| t.to_rfc3339()),
            })
            .collect();
        Ok(entries)
    }
}

/// Parse an optional "start-end" line-range selector (1-indexed, inclusive).
/// Returns a 0-indexed `[start, end)` range clamped to `[0, total_lines]`.
fn parse_selector(selector: Option<&str>, total_lines: usize) -> (usize, usize) {
    const DEFAULT_PAGE: usize = 50;
    let end_default = total_lines.min(DEFAULT_PAGE);
    let Some(sel) = selector else {
        return (0, end_default);
    };
    let Some((a, b)) = sel.split_once('-') else {
        return (0, end_default);
    };
    let start = a.parse::<usize>().unwrap_or(1).saturating_sub(1);
    let end = b.parse::<usize>().unwrap_or(DEFAULT_PAGE).min(total_lines);
    (start.min(total_lines), end.max(start).min(total_lines))
}
