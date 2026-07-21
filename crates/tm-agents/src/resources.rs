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
        ctx: &InvocationCtx,
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
            .get_for_session(&ctx.session_id, &actor_id)
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

    async fn list(&self, _uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let records = self.roster.list_for_session(&ctx.session_id).await;
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
        ctx: &InvocationCtx,
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
            .get_for_session(&ctx.session_id, &actor_id)
            .await
            .ok_or_else(|| HostError::NotFound(format!("actor {id_str} not found")))?;

        let full = self
            .roster
            .get_transcript_for_session(&ctx.session_id, &actor_id)
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

    async fn list(&self, _uri: Option<&str>, ctx: &InvocationCtx) -> Result<Vec<ResourceEntry>> {
        let entries = self
            .roster
            .list_for_session(&ctx.session_id)
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use tm_host::{CapabilityGrants, HostError, InvocationCtx, ResourceHandler};

    use super::*;
    use crate::{
        ActorBudget, ActorId,
        actor::{ActorRecord, ActorStatus},
    };

    fn ctx() -> InvocationCtx {
        InvocationCtx::new(CapabilityGrants::default())
    }

    fn ctx_for(session_id: &str) -> InvocationCtx {
        InvocationCtx::new(CapabilityGrants::default()).with_session_id(session_id)
    }

    fn test_record(id: &str, mode: Option<&str>, history_uri: Option<&str>) -> ActorRecord {
        ActorRecord {
            id: ActorId::new(id).expect("valid actor id"),
            parent: None,
            status: ActorStatus::Running,
            mode: mode.map(str::to_string),
            budget: ActorBudget::default(),
            spawned_at: Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
            artifact_uri: None,
            history_uri: history_uri.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn agent_resource_reads_record_json_and_lists_records() {
        let roster = Arc::new(MailboxRegistry::new());
        roster
            .track_for_session("", test_record("Worker", Some("researcher"), None))
            .await
            .unwrap();
        roster
            .track_for_session("", test_record("Reviewer", Some("critic"), None))
            .await
            .unwrap();
        let handler = AgentResourceHandler::new(roster);

        assert_eq!(handler.scheme(), "agent");
        assert_eq!(handler.capability(), CAP_AGENT_READ);

        let content = handler
            .read("agent://Worker", None, &ctx())
            .await
            .expect("read actor resource");
        let record: ActorRecord =
            serde_json::from_str(&content.content).expect("actor record json");

        assert_eq!(content.uri, "agent://Worker");
        assert_eq!(content.kind, "actor");
        assert_eq!(content.mime, "application/json");
        assert_eq!(content.title.as_deref(), Some("Actor Worker"));
        assert_eq!(content.size_bytes, content.content.len());
        assert_eq!(record.id, ActorId::new("Worker").unwrap());
        assert_eq!(record.mode.as_deref(), Some("researcher"));

        let entries = handler.list(None, &ctx()).await.expect("list agents");
        assert_eq!(entries.len(), 2);
        let worker = entries
            .iter()
            .find(|entry| entry.uri == "agent://Worker")
            .expect("worker entry");
        assert_eq!(worker.name, "Worker");
        assert_eq!(worker.kind, "actor");
        assert_eq!(worker.title.as_deref(), Some("researcher"));
    }

    #[tokio::test]
    async fn agent_resource_rejects_bad_or_missing_actor_uris() {
        let roster = Arc::new(MailboxRegistry::new());
        let handler = AgentResourceHandler::new(roster);

        let empty = handler.read("agent://", None, &ctx()).await.unwrap_err();
        assert!(matches!(empty, HostError::InvalidArgs(_)));

        let invalid_id = handler
            .read("agent://lowercase", None, &ctx())
            .await
            .unwrap_err();
        assert!(matches!(invalid_id, HostError::NotFound(_)));

        let missing = handler
            .read("agent://Missing", None, &ctx())
            .await
            .unwrap_err();
        assert!(matches!(missing, HostError::NotFound(_)));
    }

    #[tokio::test]
    async fn history_resource_reads_selected_transcript_lines_and_lists_only_history_records() {
        let roster = Arc::new(MailboxRegistry::new());
        let worker = ActorId::new("Worker").unwrap();
        roster
            .track_for_session(
                "",
                test_record("Worker", Some("researcher"), Some("history://Worker")),
            )
            .await
            .unwrap();
        roster
            .track_for_session("", test_record("NoHistory", Some("observer"), None))
            .await
            .unwrap();
        roster
            .store_transcript_for_session(
                "",
                &worker,
                "line 1\nline 2\nline 3\nline 4\nline 5".to_string(),
            )
            .await;
        let handler = HistoryResourceHandler::new(roster);

        assert_eq!(handler.scheme(), "history");
        assert_eq!(handler.capability(), CAP_HISTORY_READ);

        let content = handler
            .read("history://Worker", Some("2-4"), &ctx())
            .await
            .expect("read history resource");

        assert_eq!(content.uri, "history://Worker");
        assert_eq!(content.kind, "history");
        assert_eq!(content.mime, "text/plain");
        assert_eq!(content.title.as_deref(), Some("researcher transcript"));
        assert_eq!(content.selector.as_deref(), Some("2-4"));
        assert_eq!(content.content, "line 2\nline 3\nline 4");
        assert!(content.has_more);
        assert_eq!(content.preview, "line 1\nline 2\nline 3\nline 4\nline 5");
        assert_eq!(content.size_bytes, content.preview.len());

        let entries = handler.list(None, &ctx()).await.expect("list histories");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uri, "history://Worker");
        assert_eq!(entries[0].kind, "history");
        assert_eq!(entries[0].title.as_deref(), Some("researcher"));
    }

    #[tokio::test]
    async fn history_resource_falls_back_when_transcript_is_missing() {
        let roster = Arc::new(MailboxRegistry::new());
        roster
            .track_for_session(
                "",
                test_record("Worker", Some("researcher"), Some("history://Worker")),
            )
            .await
            .unwrap();
        let handler = HistoryResourceHandler::new(roster);

        let content = handler
            .read("history://Worker", None, &ctx())
            .await
            .expect("read fallback history");

        assert!(content.content.contains("no transcript captured yet"));
        assert!(!content.has_more);
        assert_eq!(content.selector, None);
    }

    #[tokio::test]
    async fn history_resource_rejects_bad_or_missing_actor_uris() {
        let roster = Arc::new(MailboxRegistry::new());
        let handler = HistoryResourceHandler::new(roster);

        let empty = handler.read("history://", None, &ctx()).await.unwrap_err();
        assert!(matches!(empty, HostError::InvalidArgs(_)));

        let invalid_id = handler
            .read("history://lowercase", None, &ctx())
            .await
            .unwrap_err();
        assert!(matches!(invalid_id, HostError::NotFound(_)));

        let missing = handler
            .read("history://Missing", None, &ctx())
            .await
            .unwrap_err();
        assert!(matches!(missing, HostError::NotFound(_)));
    }

    #[tokio::test]
    async fn actor_and_history_resources_are_exactly_session_scoped() {
        let roster = Arc::new(MailboxRegistry::new());
        let worker = ActorId::new("Worker").unwrap();
        roster
            .track_for_session(
                "session-a",
                test_record("Worker", Some("alpha"), Some("history://Worker")),
            )
            .await
            .unwrap();
        roster
            .track_for_session(
                "session-b",
                test_record("Worker", Some("beta"), Some("history://Worker")),
            )
            .await
            .unwrap();
        roster
            .track_for_session("session-b", test_record("OnlyB", Some("private"), None))
            .await
            .unwrap();
        roster
            .store_transcript_for_session("session-a", &worker, "alpha history".to_string())
            .await;
        roster
            .store_transcript_for_session("session-b", &worker, "beta history".to_string())
            .await;

        let agents = AgentResourceHandler::new(Arc::clone(&roster));
        let histories = HistoryResourceHandler::new(roster);
        let alpha = ctx_for("session-a");

        let actor = agents.read("agent://Worker", None, &alpha).await.unwrap();
        let record: ActorRecord = serde_json::from_str(&actor.content).unwrap();
        assert_eq!(record.mode.as_deref(), Some("alpha"));
        assert_eq!(agents.list(None, &alpha).await.unwrap().len(), 1);
        assert!(matches!(
            agents.read("agent://OnlyB", None, &alpha).await,
            Err(HostError::NotFound(_))
        ));

        let history = histories
            .read("history://Worker", None, &alpha)
            .await
            .unwrap();
        assert_eq!(history.content, "alpha history");
        assert!(!history.content.contains("beta"));
        assert_eq!(histories.list(None, &alpha).await.unwrap().len(), 1);
    }

    #[test]
    fn history_selector_defaults_and_clamps_ranges() {
        assert_eq!(parse_selector(None, 120), (0, 50));
        assert_eq!(parse_selector(None, 7), (0, 7));
        assert_eq!(parse_selector(Some("not-a-range"), 120), (0, 50));
        assert_eq!(parse_selector(Some("3-nope"), 120), (2, 50));
        assert_eq!(parse_selector(Some("0-3"), 120), (0, 3));
        assert_eq!(parse_selector(Some("4-2"), 120), (3, 3));
        assert_eq!(parse_selector(Some("99-150"), 120), (98, 120));
        assert_eq!(parse_selector(Some("200-300"), 120), (120, 120));
    }
}
