use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::actor::{ActorId, ActorLifecycleEvent, ActorRecord, ActorStatus};
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

/// Maximum number of messages retained in the in-memory log.
/// Older messages are dropped (oldest-first) once this limit is exceeded.
const MAX_MESSAGES: usize = 1000;
const MAX_INBOX_MESSAGES: usize = 64;

struct ActorInbox {
    sender: mpsc::Sender<ActorMessage>,
    receiver: Mutex<mpsc::Receiver<ActorMessage>>,
    backlog: Mutex<VecDeque<ActorMessage>>,
    unread: AtomicU64,
    last_activity: RwLock<Option<DateTime<Utc>>>,
}

impl ActorInbox {
    fn new() -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(MAX_INBOX_MESSAGES);
        Arc::new(Self {
            sender,
            receiver: Mutex::new(receiver),
            backlog: Mutex::new(VecDeque::new()),
            unread: AtomicU64::new(0),
            last_activity: RwLock::new(None),
        })
    }

    async fn touch(&self, at: DateTime<Utc>) {
        *self.last_activity.write().await = Some(at);
    }

    async fn last_activity(&self) -> Option<DateTime<Utc>> {
        self.last_activity.read().await.clone()
    }

    fn unread(&self) -> usize {
        self.unread.load(Ordering::Relaxed) as usize
    }

    fn mark_delivered_to_inbox(&self) {
        self.unread.fetch_add(1, Ordering::Relaxed);
    }

    fn mark_taken_by_actor(&self) {
        let _ = self
            .unread
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(1))
            });
    }

    async fn drain(&self, from: Option<&ActorId>) -> Vec<ActorMessage> {
        let mut drained = Vec::new();

        {
            let mut backlog = self.backlog.lock().await;
            let mut retained = VecDeque::new();
            while let Some(message) = backlog.pop_front() {
                if message_matches_from(&message, from) {
                    self.mark_taken_by_actor();
                    drained.push(message);
                } else {
                    retained.push_back(message);
                }
            }
            *backlog = retained;
        }

        let mut receiver = self.receiver.lock().await;
        loop {
            match receiver.try_recv() {
                Ok(message) if message_matches_from(&message, from) => {
                    self.mark_taken_by_actor();
                    drained.push(message);
                }
                Ok(message) => {
                    self.backlog.lock().await.push_back(message);
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }

        drained
    }

    async fn wait(&self, from: Option<&ActorId>, timeout: Duration) -> Option<ActorMessage> {
        if let Some(message) = self.take_matching_backlog(from).await {
            return Some(message);
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline.saturating_duration_since(now);
            let received = {
                let mut receiver = self.receiver.lock().await;
                tokio::time::timeout(remaining, receiver.recv()).await
            };
            match received {
                Ok(Some(message)) if message_matches_from(&message, from) => {
                    self.mark_taken_by_actor();
                    return Some(message);
                }
                Ok(Some(message)) => {
                    self.backlog.lock().await.push_back(message);
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }

    async fn take_matching_backlog(&self, from: Option<&ActorId>) -> Option<ActorMessage> {
        let mut backlog = self.backlog.lock().await;
        let index = backlog
            .iter()
            .position(|message| message_matches_from(message, from))?;
        let message = backlog.remove(index)?;
        drop(backlog);
        self.mark_taken_by_actor();
        Some(message)
    }
}

fn message_matches_from(message: &ActorMessage, from: Option<&ActorId>) -> bool {
    match from {
        Some(from) => &message.from == from,
        None => true,
    }
}

/// Concurrent roster of live actors, plus the injected executor for `agents.run` (P3.1).
///
/// Keeps both a bounded replay/debug message log and live bounded per-actor inbox queues.
/// Completed actors retain their last digest summary so `agents.msg` can still use the
/// P3 compatibility seeded-continuation path when a target is no longer running.
pub struct MailboxRegistry {
    actors: RwLock<HashMap<ActorId, ActorRecord>>,
    inboxes: RwLock<HashMap<ActorId, Arc<ActorInbox>>>,
    executor: std::sync::OnceLock<Arc<dyn ActorExecutor>>,
    next_seq: AtomicU64,
    messages: RwLock<Vec<ActorMessage>>,
    /// In-memory actor transcripts keyed by actor id (P3.3).
    /// Populated by ChatActorExecutor after each run; served by HistoryResourceHandler.
    transcripts: RwLock<HashMap<ActorId, String>>,
    /// Optional lifecycle event hook (P3.5).
    ///
    /// Set once at server startup via `set_lifecycle_hook`. When set, `emit_lifecycle` calls
    /// the hook synchronously — the hook implementation is responsible for routing the event
    /// to the right session SSE stream (typically via `tokio::runtime::Handle::spawn`).
    lifecycle_hook: std::sync::OnceLock<Box<dyn Fn(String, ActorLifecycleEvent) + Send + Sync>>,
}

impl MailboxRegistry {
    pub fn new() -> Self {
        Self {
            actors: RwLock::new(HashMap::new()),
            inboxes: RwLock::new(HashMap::new()),
            executor: std::sync::OnceLock::new(),
            next_seq: AtomicU64::new(0),
            messages: RwLock::new(Vec::new()),
            transcripts: RwLock::new(HashMap::new()),
            lifecycle_hook: std::sync::OnceLock::new(),
        }
    }

    /// Install the lifecycle event hook (P3.5). Called once at server startup.
    ///
    /// The hook is called synchronously from `emit_lifecycle`; async work (e.g. store writes)
    /// must be dispatched inside the hook via a captured `tokio::runtime::Handle`.
    pub fn set_lifecycle_hook(
        &self,
        hook: impl Fn(String, ActorLifecycleEvent) + Send + Sync + 'static,
    ) {
        let _ = self.lifecycle_hook.set(Box::new(hook));
    }

    /// Fire the lifecycle hook if one is installed (no-op otherwise).
    pub fn emit_lifecycle(&self, session_id: &str, event: ActorLifecycleEvent) {
        if let Some(hook) = self.lifecycle_hook.get() {
            hook(session_id.to_string(), event);
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
        self.ensure_inbox(&record.id).await;
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

    /// Mark an actor as successfully terminated and store its full digest (P3.3).
    ///
    /// Stores summary (seeds `agents.msg` continuations), artifact URI, and history URI.
    pub async fn mark_complete_with_digest(
        &self,
        id: &ActorId,
        summary: String,
        artifact_uri: Option<String>,
        history_uri: Option<String>,
    ) {
        if let Some(rec) = self.actors.write().await.get_mut(id) {
            rec.status = ActorStatus::Terminated;
            rec.completed_at = Some(Utc::now());
            rec.last_summary = Some(summary);
            rec.artifact_uri = artifact_uri;
            rec.history_uri = history_uri;
        }
    }

    /// Store the full transcript for an actor (P3.3).
    ///
    /// Called by the orchestrator after `run_to_digest` returns `history_content`.
    /// Served by `HistoryResourceHandler`.
    pub async fn store_transcript(&self, id: &ActorId, content: String) {
        self.transcripts.write().await.insert(id.clone(), content);
    }

    /// Retrieve the stored transcript for an actor, if any.
    pub async fn get_transcript(&self, id: &ActorId) -> Option<String> {
        self.transcripts.read().await.get(id).cloned()
    }

    /// Append a message to the bounded in-memory log.
    ///
    /// Oldest messages are dropped once the log exceeds [`MAX_MESSAGES`].
    pub async fn record_message(&self, msg: ActorMessage) {
        let mut msgs = self.messages.write().await;
        if msgs.len() >= MAX_MESSAGES {
            let excess = msgs.len() - MAX_MESSAGES + 1;
            msgs.drain(0..excess);
        }
        msgs.push(msg);
    }

    /// Deliver a message to the recipient's live inbox and append it to the bounded log.
    ///
    /// Unknown or terminated actors return [`Receipt::Failed`]. The synthetic `Root`
    /// actor is always reachable so child actors can reply to the top-level orchestrator.
    pub async fn send_message(&self, msg: ActorMessage) -> Receipt {
        if !self.is_reachable(&msg.to).await {
            return Receipt::Failed;
        }

        let inbox = self.ensure_inbox(&msg.to).await;
        match inbox.sender.try_send(msg.clone()) {
            Ok(()) => {
                inbox.mark_delivered_to_inbox();
                inbox.touch(msg.sent_at).await;
                self.record_message(msg).await;
                Receipt::Delivered
            }
            Err(_) => Receipt::Failed,
        }
    }

    /// Drain all pending messages for `actor_id`, optionally filtering by sender.
    pub async fn drain_inbox(
        &self,
        actor_id: &ActorId,
        from: Option<&ActorId>,
    ) -> Vec<ActorMessage> {
        let inbox = self.ensure_inbox(actor_id).await;
        inbox.drain(from).await
    }

    /// Wait for the next matching message for `actor_id` until `timeout` elapses.
    pub async fn wait_for_message(
        &self,
        actor_id: &ActorId,
        from: Option<&ActorId>,
        timeout: Duration,
    ) -> Option<ActorMessage> {
        let inbox = self.ensure_inbox(actor_id).await;
        inbox.wait(from, timeout).await
    }

    pub async fn unread_count(&self, actor_id: &ActorId) -> usize {
        let Some(inbox) = self.inboxes.read().await.get(actor_id).cloned() else {
            return 0;
        };
        inbox.unread()
    }

    pub async fn last_activity(&self, actor_id: &ActorId) -> Option<DateTime<Utc>> {
        let Some(inbox) = self.inboxes.read().await.get(actor_id).cloned() else {
            return None;
        };
        inbox.last_activity().await
    }

    /// Snapshot of all messages in the log (oldest-first).
    pub async fn messages(&self) -> Vec<ActorMessage> {
        self.messages.read().await.clone()
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

    async fn ensure_inbox(&self, actor_id: &ActorId) -> Arc<ActorInbox> {
        if let Some(inbox) = self.inboxes.read().await.get(actor_id).cloned() {
            return inbox;
        }
        let mut inboxes = self.inboxes.write().await;
        inboxes
            .entry(actor_id.clone())
            .or_insert_with(ActorInbox::new)
            .clone()
    }

    async fn is_reachable(&self, actor_id: &ActorId) -> bool {
        if actor_id.as_str() == "Root" {
            return true;
        }
        self.actors
            .read()
            .await
            .get(actor_id)
            .is_some_and(|record| record.status != ActorStatus::Terminated)
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
            .finish_non_exhaustive() // actors + messages excluded (async lock)
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
            last_summary: None,
            artifact_uri: None,
            history_uri: None,
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
        assert!(
            s.chars().all(|c| c.is_alphanumeric()),
            "must be alphanumeric: {s}"
        );
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

    #[tokio::test]
    async fn mark_complete_with_digest_stores_uris() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;
        registry
            .mark_complete_with_digest(
                &id,
                "summary".to_string(),
                Some("artifact://1".to_string()),
                Some("history://Worker".to_string()),
            )
            .await;
        let rec = registry.get(&id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Terminated);
        assert_eq!(rec.last_summary.as_deref(), Some("summary"));
        assert_eq!(rec.artifact_uri.as_deref(), Some("artifact://1"));
        assert_eq!(rec.history_uri.as_deref(), Some("history://Worker"));
    }

    #[tokio::test]
    async fn store_and_get_transcript() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        assert!(registry.get_transcript(&id).await.is_none());
        registry
            .store_transcript(&id, "line 1\nline 2\n".to_string())
            .await;
        let content = registry.get_transcript(&id).await.unwrap();
        assert!(content.contains("line 1"));
    }

    #[tokio::test]
    async fn send_message_delivers_to_actor_inbox() {
        let registry = MailboxRegistry::new();
        let from = ActorId::new("Root").unwrap();
        let to = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;

        let receipt = registry
            .send_message(ActorMessage {
                from: from.clone(),
                to: to.clone(),
                text: "status?".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        assert_eq!(receipt, Receipt::Delivered);
        assert_eq!(registry.unread_count(&to).await, 1);
        let drained = registry.drain_inbox(&to, None).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].from, from);
        assert_eq!(drained[0].text, "status?");
        assert_eq!(registry.unread_count(&to).await, 0);
    }

    #[tokio::test]
    async fn send_message_to_terminated_actor_fails() {
        let registry = MailboxRegistry::new();
        let to = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;
        registry.mark_complete(&to).await;

        let receipt = registry
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: to.clone(),
                text: "status?".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        assert_eq!(receipt, Receipt::Failed);
        assert!(registry.drain_inbox(&to, None).await.is_empty());
    }

    #[tokio::test]
    async fn wait_for_message_filters_by_sender_without_losing_others() {
        let registry = MailboxRegistry::new();
        let worker = ActorId::new("Worker").unwrap();
        let alpha = ActorId::new("Alpha").unwrap();
        let beta = ActorId::new("Beta").unwrap();
        registry.track(test_record("Worker")).await;
        registry.track(test_record("Alpha")).await;
        registry.track(test_record("Beta")).await;

        for (from, text) in [(alpha.clone(), "from alpha"), (beta.clone(), "from beta")] {
            assert_eq!(
                registry
                    .send_message(ActorMessage {
                        from,
                        to: worker.clone(),
                        text: text.to_string(),
                        reply_to: None,
                        sent_at: Utc::now(),
                    })
                    .await,
                Receipt::Delivered
            );
        }

        let beta_msg = registry
            .wait_for_message(&worker, Some(&beta), Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(beta_msg.text, "from beta");

        let remaining = registry.drain_inbox(&worker, None).await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].from, alpha);
        assert_eq!(remaining[0].text, "from alpha");
    }
}
