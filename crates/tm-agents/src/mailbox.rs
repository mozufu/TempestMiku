use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::actor::{ActorId, ActorRecord, ActorStatus};
use crate::executor::ActorExecutor;
use crate::supervise::FailureReason;

/// A plain-prose message between actors (§23.2).
///
/// Protocol invariants:
/// - Plain prose only — never control-payload blobs (`{"type":"done"}` is banned).
/// - One ask per message; lead with the answer when replying; set `reply_to`.
/// - Pass large payloads by reference (`artifact://`, `memory://`), never inline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorMessage {
    pub from: ActorId,
    pub to: ActorId,
    /// Plain-prose body. The message is the interface.
    pub text: String,
    /// Set for request/reply; recipient echoes this in its reply.
    pub reply_to: Option<ActorId>,
    pub sent_at: DateTime<Utc>,
}

/// Delivery confirmation for a sent message (§23.2).
///
/// A `Failed` receipt means the actor is unreachable — sender moves on, no retry-loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Receipt {
    Delivered,
    /// Actor unreachable. Sender must not retry-loop.
    Failed,
}

/// Concurrent roster of live actors, plus the injected executor for `agents.run` (P3.1).
///
/// The mailbox channel layer (bounded MPSC queues, `CancelToken`) is deferred to P3.2
/// when `agents.spawn` and `agents.msg` have concrete callers.
pub struct MailboxRegistry {
    actors: RwLock<HashMap<ActorId, ActorRecord>>,
    executor: std::sync::OnceLock<Arc<dyn ActorExecutor>>,
    next_seq: AtomicU64,
}

impl MailboxRegistry {
    pub fn new() -> Self {
        Self {
            actors: RwLock::new(HashMap::new()),
            executor: std::sync::OnceLock::new(),
            next_seq: AtomicU64::new(0),
        }
    }

    /// Inject the executor at startup (called once by `tm-server`'s `build_runtime`).
    pub fn set_executor(&self, executor: Arc<dyn ActorExecutor>) {
        let _ = self.executor.set(executor);
    }

    /// Returns the injected executor, if any.
    pub fn executor(&self) -> Option<Arc<dyn ActorExecutor>> {
        self.executor.get().cloned()
    }

    /// Generate the next unique CamelCase actor ID based on the role string.
    pub fn next_actor_id(&self, role: &str) -> ActorId {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let seq_str = seq.to_string();
        // Build CamelCase prefix: uppercase first alphabetic char, then up to 15 more alphanumeric.
        let mut chars = role.chars().filter(|c| c.is_alphabetic());
        let first = chars
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "A".to_string());
        let rest: String = chars.take(15).collect();
        let prefix = format!("{first}{rest}");
        // Trim prefix so total length ≤ 32 chars.
        let max_prefix = 32usize.saturating_sub(seq_str.len());
        let prefix: String = prefix.chars().take(max_prefix).collect();
        let full = format!("{prefix}{seq_str}");
        // Fallback to A<seq> if the trimmed prefix is empty or invalid.
        ActorId::new(&full)
            .unwrap_or_else(|_| ActorId::new(format!("A{seq_str}")).expect("A<seq> always valid"))
    }

    /// Register an actor in the roster.
    pub async fn track(&self, record: ActorRecord) {
        self.actors.write().await.insert(record.id.clone(), record);
    }

    /// Look up a single actor record by id.
    pub async fn get(&self, id: &ActorId) -> Option<ActorRecord> {
        self.actors.read().await.get(id).cloned()
    }

    /// Update an actor's status in place.
    pub async fn update_status(&self, id: &ActorId, status: ActorStatus) {
        if let Some(rec) = self.actors.write().await.get_mut(id) {
            rec.status = status;
        }
    }

    /// Mark an actor as successfully terminated.
    pub async fn mark_complete(&self, id: &ActorId) {
        if let Some(rec) = self.actors.write().await.get_mut(id) {
            rec.status = ActorStatus::Terminated;
            rec.completed_at = Some(Utc::now());
        }
    }

    /// Mark an actor as terminated due to a failure.
    pub async fn mark_failed(&self, id: &ActorId, reason: FailureReason) {
        if let Some(rec) = self.actors.write().await.get_mut(id) {
            rec.status = ActorStatus::Terminated;
            rec.completed_at = Some(Utc::now());
            rec.cancelled = matches!(reason, FailureReason::Cancelled);
            rec.failure_reason = Some(reason);
        }
    }

    /// Snapshot of all tracked actor records.
    pub async fn list(&self) -> Vec<ActorRecord> {
        self.actors.read().await.values().cloned().collect()
    }
}

impl Default for MailboxRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MailboxRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailboxRegistry")
            .field("executor_set", &self.executor.get().is_some())
            .field("next_seq", &self.next_seq.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorBudget, ActorId};

    fn test_record(id: &str) -> ActorRecord {
        ActorRecord {
            id: ActorId::new(id).unwrap(),
            parent: None,
            status: ActorStatus::Running,
            mode: None,
            budget: ActorBudget::default(),
            spawned_at: Utc::now(),
            completed_at: None,
            cancelled: false,
            failure_reason: None,
        }
    }

    #[tokio::test]
    async fn track_and_get() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;
        let got = registry.get(&id).await.unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.status, ActorStatus::Running);
    }

    #[tokio::test]
    async fn list_returns_all() {
        let registry = MailboxRegistry::new();
        registry.track(test_record("Alpha")).await;
        registry.track(test_record("Beta")).await;
        let list = registry.list().await;
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn mark_complete_updates_status() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;
        registry.mark_complete(&id).await;
        let rec = registry.get(&id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Terminated);
        assert!(rec.completed_at.is_some());
        assert!(!rec.cancelled);
    }

    #[tokio::test]
    async fn mark_failed_records_reason() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;
        registry.mark_failed(&id, FailureReason::Timeout).await;
        let rec = registry.get(&id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Terminated);
        assert!(!rec.cancelled);
        assert!(matches!(rec.failure_reason, Some(FailureReason::Timeout)));
    }

    #[tokio::test]
    async fn mark_failed_cancelled_sets_flag() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;
        registry.mark_failed(&id, FailureReason::Cancelled).await;
        let rec = registry.get(&id).await.unwrap();
        assert!(rec.cancelled);
    }

    #[test]
    fn next_actor_id_camel_case_from_role() {
        let registry = MailboxRegistry::new();
        let id = registry.next_actor_id("researcher");
        let s = id.as_str();
        assert!(s.starts_with('R'), "should start with 'R', got {s}");
        assert!(s.chars().all(|c| c.is_alphanumeric()), "must be alphanumeric: {s}");
        assert!(s.len() <= 32, "must be ≤32 chars: {s}");
    }

    #[test]
    fn next_actor_id_incrementing_suffix() {
        let registry = MailboxRegistry::new();
        let a = registry.next_actor_id("worker");
        let b = registry.next_actor_id("worker");
        assert_ne!(a, b);
    }

    #[test]
    fn next_actor_id_empty_role_fallback() {
        let registry = MailboxRegistry::new();
        let id = registry.next_actor_id("");
        assert!(id.as_str().starts_with('A'));
    }

    #[test]
    fn executor_not_set_returns_none() {
        let registry = MailboxRegistry::new();
        assert!(registry.executor().is_none());
    }
}
