use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tm_core::CancellationToken;
use tm_host::CapabilityGrants;
use tokio::sync::Notify;

use crate::supervise::{FailureReason, SupervisionDecision};

/// Maximum UTF-8 bytes accepted for an actor role or mode label.
pub const MAX_ACTOR_ROLE_BYTES: usize = 128;
/// Maximum UTF-8 bytes accepted for one actor task.
pub const MAX_ACTOR_TASK_BYTES: usize = 8 * 1024;
/// Maximum UTF-8 bytes accepted for one mailbox message.
pub const MAX_ACTOR_MESSAGE_BYTES: usize = 4 * 1024;
/// Maximum UTF-8 bytes returned to a parent as the inline actor digest summary.
pub const MAX_ACTOR_SUMMARY_BYTES: usize = 4 * 1024;
/// Maximum UTF-8 bytes retained for an in-memory actor transcript/history resource.
pub const MAX_ACTOR_TRANSCRIPT_BYTES: usize = 64 * 1024;
pub const ACTOR_TEXT_TRUNCATED_MARKER: &str = "\n[actor output truncated]";

/// Shareable cancellation token for a single actor run.
#[derive(Debug, Clone, Default)]
pub struct ActorCancelToken {
    cancelled: Arc<AtomicBool>,
    notification: Arc<Notify>,
}

impl ActorCancelToken {
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            self.notification.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl CancellationToken for ActorCancelToken {
    fn is_cancelled(&self) -> bool {
        ActorCancelToken::is_cancelled(self)
    }

    fn cancelled(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            loop {
                let notified = self.notification.notified();
                if self.is_cancelled() {
                    return;
                }
                notified.await;
            }
        })
    }
}

/// Stable actor identity. CamelCase (starts with uppercase, alphanumeric only), ≤32 chars (§23.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActorId(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActorIdError {
    #[error("actor id must be ≤32 characters, got {0}")]
    TooLong(usize),
    #[error("actor id must be CamelCase (start uppercase, alphanumeric only), got {0:?}")]
    InvalidFormat(String),
}

impl ActorId {
    pub fn new(id: impl Into<String>) -> Result<Self, ActorIdError> {
        let id = id.into();
        if id.len() > 32 {
            return Err(ActorIdError::TooLong(id.len()));
        }
        if !is_camel_case(&id) {
            return Err(ActorIdError::InvalidFormat(id));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_camel_case(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    first.is_uppercase() && s.chars().all(|c| c.is_alphanumeric())
}

impl std::fmt::Display for ActorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorStatus {
    Running,
    Idle,
    Parked,
    Terminated,
}

/// Per-actor resource caps (§23.4). Conservative defaults; measure before loosening.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorBudget {
    /// Wall-clock limit in milliseconds.
    pub wall_ms: u64,
    /// Maximum spawn depth from this actor; prevents runaway recursion.
    pub max_depth: u32,
}

impl Default for ActorBudget {
    fn default() -> Self {
        Self {
            wall_ms: 120_000,
            max_depth: 4,
        }
    }
}

/// Configuration for spawning an actor (§23.1).
#[derive(Debug, Clone)]
pub struct ActorSpec {
    pub id: ActorId,
    /// Parent session UUID string used for replayable SSE/resource routing.
    ///
    /// Child actors still own their private model context, but effects they surface
    /// to the user (approval prompts, lifecycle events, and artifacts) belong to
    /// the spawning session.
    pub session_id: String,
    /// Exact server-authoritative session scope inherited from the spawning host call.
    pub session_scope: Option<String>,
    pub role: String,
    pub task: String,
    pub mode: Option<String>,
    pub grants: CapabilityGrants,
    pub budget: ActorBudget,
    pub parent: Option<ActorId>,
    /// Current recursion depth from the root session (enforced against budget.max_depth).
    pub depth: u32,
    /// Token shared between parent cancellation, the actor loop, and the sandbox backend.
    pub cancellation: ActorCancelToken,
}

impl ActorSpec {
    /// Validate all model-controlled text before an executor allocates a prompt or worker.
    pub fn validate_text_limits(&self) -> Result<(), String> {
        validate_text_bytes("role", &self.role, MAX_ACTOR_ROLE_BYTES)?;
        validate_text_bytes("task", &self.task, MAX_ACTOR_TASK_BYTES)?;
        if let Some(mode) = self.mode.as_deref() {
            validate_text_bytes("mode", mode, MAX_ACTOR_ROLE_BYTES)?;
        }
        Ok(())
    }
}

pub(crate) fn validate_text_bytes(field: &str, text: &str, max_bytes: usize) -> Result<(), String> {
    if text.len() > max_bytes {
        return Err(format!(
            "{field} must be at most {max_bytes} UTF-8 bytes, got {}",
            text.len()
        ));
    }
    Ok(())
}

/// Full lifecycle record persisted in the session event log (§23.4, P3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorRecord {
    pub id: ActorId,
    pub parent: Option<ActorId>,
    pub status: ActorStatus,
    pub mode: Option<String>,
    pub budget: ActorBudget,
    pub spawned_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled: bool,
    pub failure_reason: Option<FailureReason>,
    /// Plain-prose digest summary stored on completion; seeds seeded continuations in agents.msg.
    pub last_summary: Option<String>,
    /// URI of the primary output artifact produced by this actor (P3.3).
    pub artifact_uri: Option<String>,
    /// URI of the read-only transcript for this actor (P3.3).
    pub history_uri: Option<String>,
}

/// Opaque handle returned by `agents.spawn` for coordination via messages (§23.3).
///
/// Handles wire the DAG by reference — large transcripts are never re-inlined.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorHandle {
    pub id: ActorId,
}

/// Bounded digest result returned to the parent context (§23.5).
///
/// Only the digest reaches the orchestrator's model context; full output lives in
/// `artifact://` and the read-only transcript in `history://`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorDigest {
    pub actor_id: ActorId,
    /// Summary text — the only part injected into parent context.
    pub summary: String,
    /// URI of the full output artifact, when the output exceeded the digest threshold.
    pub artifact_uri: Option<String>,
    /// URI of the read-only transcript for this actor.
    pub history_uri: Option<String>,
    /// Full transcript content captured by CollectingSink (P3.3).
    /// Stripped before the digest JSON is sent to the model; stored in MailboxRegistry.transcripts.
    #[serde(skip)]
    pub history_content: Option<String>,
}

impl ActorDigest {
    pub fn enforce_text_limits(&mut self) {
        self.summary = truncate_utf8_with_marker(&self.summary, MAX_ACTOR_SUMMARY_BYTES);
        self.history_content = self
            .history_content
            .take()
            .map(|content| truncate_utf8_with_marker(&content, MAX_ACTOR_TRANSCRIPT_BYTES));
    }
}

pub fn truncate_utf8_with_marker(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    if max_bytes <= ACTOR_TEXT_TRUNCATED_MARKER.len() {
        return ACTOR_TEXT_TRUNCATED_MARKER[..max_bytes].to_string();
    }
    let content_limit = max_bytes - ACTOR_TEXT_TRUNCATED_MARKER.len();
    let mut end = content_limit.min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let mut bounded = String::with_capacity(max_bytes);
    bounded.push_str(&value[..end]);
    bounded.push_str(ACTOR_TEXT_TRUNCATED_MARKER);
    bounded
}

/// Actor lifecycle events for the session event log (§23.4, P3.1).
///
/// Serialize to `payload_json` in `SessionEvent`; `event_type()` is the discriminant string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActorLifecycleEvent {
    Spawned {
        actor_id: ActorId,
        parent_id: Option<ActorId>,
        role: String,
        task: String,
        depth: u32,
        budget: ActorBudget,
        spawned_at: DateTime<Utc>,
    },
    StatusChanged {
        actor_id: ActorId,
        status: ActorStatus,
        at: DateTime<Utc>,
    },
    MessageSent {
        from: ActorId,
        to: ActorId,
        reply_to: Option<ActorId>,
        sent_at: DateTime<Utc>,
    },
    Completed {
        actor_id: ActorId,
        completed_at: DateTime<Utc>,
        summary: Option<String>,
        artifact_uri: Option<String>,
        history_uri: Option<String>,
    },
    Failed {
        actor_id: ActorId,
        failed_at: DateTime<Utc>,
        reason: FailureReason,
    },
    Supervision {
        supervisor_id: ActorId,
        failed_actor_id: ActorId,
        decision: SupervisionDecision,
        decided_at: DateTime<Utc>,
    },
    Cancelled {
        actor_id: ActorId,
        cancelled_at: DateTime<Utc>,
    },
}

/// Resource handles produced by a completed actor and linked into the parent session log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorOutputLink {
    pub actor_id: ActorId,
    pub artifact_uri: Option<String>,
    pub history_uri: Option<String>,
}

impl ActorLifecycleEvent {
    /// The `event_type` string written to the session event log.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::Spawned { .. } => "actor_spawned",
            Self::StatusChanged { .. } => "actor_status",
            Self::MessageSent { .. } => "actor_message",
            Self::Completed { .. } => "actor_completed",
            Self::Failed { .. } => "actor_failed",
            Self::Supervision { .. } => "actor_supervision",
            Self::Cancelled { .. } => "actor_cancelled",
        }
    }

    /// Child output handles that should be queryable from the parent session event log.
    pub fn output_link(&self) -> Option<ActorOutputLink> {
        match self {
            Self::Completed {
                actor_id,
                artifact_uri,
                history_uri,
                ..
            } if artifact_uri.is_some() || history_uri.is_some() => Some(ActorOutputLink {
                actor_id: actor_id.clone(),
                artifact_uri: artifact_uri.clone(),
                history_uri: history_uri.clone(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_id_rejects_too_long() {
        let long = "A".repeat(33);
        assert_eq!(ActorId::new(&long), Err(ActorIdError::TooLong(33)));
    }

    #[test]
    fn actor_id_rejects_lowercase_start() {
        assert!(matches!(
            ActorId::new("worker"),
            Err(ActorIdError::InvalidFormat(_))
        ));
    }

    #[test]
    fn actor_id_rejects_non_alphanumeric() {
        assert!(matches!(
            ActorId::new("My-Worker"),
            Err(ActorIdError::InvalidFormat(_))
        ));
        assert!(matches!(
            ActorId::new("My_Worker"),
            Err(ActorIdError::InvalidFormat(_))
        ));
    }

    #[test]
    fn completed_lifecycle_event_exposes_output_link() {
        let actor_id = ActorId::new("Worker").unwrap();
        let event = ActorLifecycleEvent::Completed {
            actor_id: actor_id.clone(),
            completed_at: Utc::now(),
            summary: Some("done".to_string()),
            artifact_uri: Some("artifact://0".to_string()),
            history_uri: Some("history://Worker".to_string()),
        };

        let link = event.output_link().expect("completed output link");

        assert_eq!(link.actor_id, actor_id);
        assert_eq!(link.artifact_uri.as_deref(), Some("artifact://0"));
        assert_eq!(link.history_uri.as_deref(), Some("history://Worker"));
    }

    #[test]
    fn lifecycle_event_without_output_has_no_output_link() {
        let event = ActorLifecycleEvent::Completed {
            actor_id: ActorId::new("Worker").unwrap(),
            completed_at: Utc::now(),
            summary: Some("done".to_string()),
            artifact_uri: None,
            history_uri: None,
        };

        assert_eq!(event.output_link(), None);
    }

    #[test]
    fn actor_id_accepts_valid_camel_case() {
        assert!(ActorId::new("Worker").is_ok());
        assert!(ActorId::new("ResearchWorker").is_ok());
        assert!(ActorId::new("A").is_ok());
        assert!(ActorId::new("A".repeat(32)).is_ok());
    }

    #[test]
    fn actor_id_rejects_empty() {
        assert!(matches!(
            ActorId::new(""),
            Err(ActorIdError::InvalidFormat(_))
        ));
    }

    #[test]
    fn actor_digest_limits_are_utf8_safe_and_mark_truncation() {
        let mut digest = ActorDigest {
            actor_id: ActorId::new("Worker").unwrap(),
            summary: "好".repeat(MAX_ACTOR_SUMMARY_BYTES),
            artifact_uri: None,
            history_uri: Some("history://Worker".to_string()),
            history_content: Some("界".repeat(MAX_ACTOR_TRANSCRIPT_BYTES)),
        };

        digest.enforce_text_limits();

        assert!(digest.summary.len() <= MAX_ACTOR_SUMMARY_BYTES);
        assert!(digest.summary.ends_with(ACTOR_TEXT_TRUNCATED_MARKER));
        let history = digest.history_content.unwrap();
        assert!(history.len() <= MAX_ACTOR_TRANSCRIPT_BYTES);
        assert!(history.ends_with(ACTOR_TEXT_TRUNCATED_MARKER));
    }
}
