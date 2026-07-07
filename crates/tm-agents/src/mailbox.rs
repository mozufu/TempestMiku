use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, mpsc};

use tm_host::CapabilityGrants;

use crate::actor::{
    ActorBudget, ActorCancelToken, ActorId, ActorLifecycleEvent, ActorRecord, ActorSpec,
    ActorStatus,
};
use crate::executor::ActorExecutor;
use crate::supervise::{
    FailureReason, SupervisionAction, SupervisionDecision, SupervisionPolicy, Supervisor,
};

type RawEventFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type RawEventHook =
    dyn Fn(String, String, serde_json::Value) -> RawEventFuture + Send + Sync + 'static;

#[derive(Debug, Clone)]
struct ActorRestartTemplate {
    id: ActorId,
    session_id: String,
    role: String,
    task: String,
    mode: Option<String>,
    grants: CapabilityGrants,
    budget: ActorBudget,
    parent: Option<ActorId>,
    supervisor_id: ActorId,
    depth: u32,
}

impl ActorRestartTemplate {
    fn from_spec(spec: &ActorSpec, supervisor_id: ActorId) -> Self {
        Self {
            id: spec.id.clone(),
            session_id: spec.session_id.clone(),
            role: spec.role.clone(),
            task: spec.task.clone(),
            mode: spec.mode.clone(),
            grants: spec.grants.clone(),
            budget: spec.budget.clone(),
            parent: spec.parent.clone(),
            supervisor_id,
            depth: spec.depth,
        }
    }

    fn to_spec(&self, cancellation: ActorCancelToken) -> ActorSpec {
        ActorSpec {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            role: self.role.clone(),
            task: self.task.clone(),
            mode: self.mode.clone(),
            grants: self.grants.clone(),
            budget: self.budget.clone(),
            parent: self.parent.clone(),
            depth: self.depth,
            cancellation,
        }
    }
}

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
/// A failed receipt means the message was not enqueued — sender moves on, no retry-loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Receipt {
    Delivered,
    /// Actor unknown, dead, or otherwise unreachable.
    Unreachable,
    /// Recipient inbox is full; this send is dropped rather than silently accepted.
    Backpressured,
}

impl Receipt {
    pub fn is_delivered(self) -> bool {
        self == Self::Delivered
    }

    pub fn is_failed(self) -> bool {
        !self.is_delivered()
    }

    pub fn failure_reason(self) -> Option<&'static str> {
        match self {
            Self::Delivered => None,
            Self::Unreachable => Some("unreachable"),
            Self::Backpressured => Some("backpressured"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelActorResult {
    Cancelled,
    AlreadyCancelled,
    AlreadyTerminated,
    NotFound,
}

impl CancelActorResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::AlreadyCancelled => "already_cancelled",
            Self::AlreadyTerminated => "already_terminated",
            Self::NotFound => "not_found",
        }
    }
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
    cancel_tokens: RwLock<HashMap<ActorId, ActorCancelToken>>,
    executor: std::sync::OnceLock<Arc<dyn ActorExecutor>>,
    next_seq: AtomicU64,
    next_supervisor_seq: AtomicU64,
    messages: RwLock<Vec<ActorMessage>>,
    /// In-memory actor transcripts keyed by actor id (P3.3).
    /// Populated by ChatActorExecutor after each run; served by HistoryResourceHandler.
    transcripts: RwLock<HashMap<ActorId, String>>,
    restart_templates: RwLock<HashMap<ActorId, ActorRestartTemplate>>,
    actor_supervisors: RwLock<HashMap<ActorId, ActorId>>,
    supervisors: RwLock<HashMap<ActorId, Supervisor>>,
    /// Optional lifecycle event hook (P3.5).
    ///
    /// Set once at server startup via `set_lifecycle_hook`. When set, `emit_lifecycle` calls
    /// the hook synchronously — the hook implementation is responsible for routing the event
    /// to the right session SSE stream (typically via `tokio::runtime::Handle::spawn`).
    lifecycle_hook: std::sync::OnceLock<Box<dyn Fn(String, ActorLifecycleEvent) + Send + Sync>>,
    /// Optional raw session-event hook for non-lifecycle actor-thread events.
    ///
    /// Child approval requests use this so they can write replayable SSE events from
    /// their own worker threads without depending on `tm-server` storage types.
    raw_event_hook: std::sync::OnceLock<Box<RawEventHook>>,
}

impl MailboxRegistry {
    pub fn new() -> Self {
        Self {
            actors: RwLock::new(HashMap::new()),
            inboxes: RwLock::new(HashMap::new()),
            cancel_tokens: RwLock::new(HashMap::new()),
            executor: std::sync::OnceLock::new(),
            next_seq: AtomicU64::new(0),
            next_supervisor_seq: AtomicU64::new(0),
            messages: RwLock::new(Vec::new()),
            transcripts: RwLock::new(HashMap::new()),
            restart_templates: RwLock::new(HashMap::new()),
            actor_supervisors: RwLock::new(HashMap::new()),
            supervisors: RwLock::new(HashMap::new()),
            lifecycle_hook: std::sync::OnceLock::new(),
            raw_event_hook: std::sync::OnceLock::new(),
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

    /// Install a raw session-event hook. Called once by the server at startup.
    ///
    /// Unlike lifecycle events, this hook is async so callers running inside actor
    /// worker runtimes can await persistence before continuing an approval flow.
    pub fn set_raw_event_hook(
        &self,
        hook: impl Fn(String, String, serde_json::Value) -> RawEventFuture + Send + Sync + 'static,
    ) {
        let _ = self.raw_event_hook.set(Box::new(hook));
    }

    /// Emit a raw session event through the configured hook.
    pub async fn emit_raw_event(
        &self,
        session_id: &str,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        let Some(hook) = self.raw_event_hook.get() else {
            return Err("raw actor event hook not configured".to_string());
        };
        hook(session_id.to_string(), event_type.to_string(), payload).await
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

    pub fn next_supervisor_id(&self, label: &str) -> ActorId {
        let seq = self.next_supervisor_seq.fetch_add(1, Ordering::Relaxed);
        let mut chars = label.chars().filter(|c| c.is_alphanumeric());
        let first = chars
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "Supervisor".to_string());
        let rest: String = chars.take(15).collect();
        ActorId::new(format!("{first}{rest}{seq}")).unwrap_or_else(|_| {
            ActorId::new(format!("Supervisor{seq}")).expect("valid supervisor id")
        })
    }

    /// Register an actor in the roster.
    pub async fn track(&self, record: ActorRecord) {
        let supervisor_id = record
            .parent
            .clone()
            .unwrap_or_else(root_supervisor_actor_id);
        self.track_with_supervisor(record, supervisor_id).await;
    }

    pub async fn track_with_supervisor(&self, record: ActorRecord, supervisor_id: ActorId) {
        let actor_id = record.id.clone();
        self.ensure_inbox(&record.id).await;
        self.ensure_cancel_token(&record.id).await;
        self.actors.write().await.insert(record.id.clone(), record);
        self.actor_supervisors
            .write()
            .await
            .insert(actor_id.clone(), supervisor_id.clone());
        self.supervisors
            .write()
            .await
            .entry(supervisor_id)
            .or_default()
            .track(actor_id);
    }

    pub async fn remember_restart_spec(&self, spec: &ActorSpec) {
        let supervisor_id = self
            .supervisor_id_for_actor(&spec.id, spec.parent.clone())
            .await;
        self.restart_templates.write().await.insert(
            spec.id.clone(),
            ActorRestartTemplate::from_spec(spec, supervisor_id),
        );
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

    pub async fn set_supervision_policy(&self, supervisor_id: ActorId, policy: SupervisionPolicy) {
        let mut supervisors = self.supervisors.write().await;
        let supervisor = supervisors
            .entry(supervisor_id)
            .or_insert_with(|| Supervisor::new(policy.clone()));
        supervisor.policy = policy;
    }

    /// Mark an actor as successfully terminated.
    pub async fn mark_complete(&self, id: &ActorId) -> bool {
        let Some(supervisor_id) = self.mark_complete_record(id, None).await else {
            return false;
        };
        self.record_supervised_success(&supervisor_id, id).await;
        true
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
    ) -> bool {
        let Some(supervisor_id) = self
            .mark_complete_record(id, Some((summary, artifact_uri, history_uri)))
            .await
        else {
            return false;
        };
        self.record_supervised_success(&supervisor_id, id).await;
        true
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
    /// Unknown or terminated actors return [`Receipt::Unreachable`]. Full inboxes return
    /// [`Receipt::Backpressured`] and drop the message. The synthetic `Root`
    /// actor is always reachable so child actors can reply to the top-level orchestrator.
    pub async fn send_message(&self, msg: ActorMessage) -> Receipt {
        if !self.is_reachable(&msg.to).await {
            return Receipt::Unreachable;
        }

        let inbox = self.ensure_inbox(&msg.to).await;
        match inbox.sender.try_send(msg.clone()) {
            Ok(()) => {
                inbox.mark_delivered_to_inbox();
                inbox.touch(msg.sent_at).await;
                self.record_message(msg).await;
                Receipt::Delivered
            }
            Err(mpsc::error::TrySendError::Full(_)) => Receipt::Backpressured,
            Err(mpsc::error::TrySendError::Closed(_)) => Receipt::Unreachable,
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
    pub async fn mark_failed(&self, id: &ActorId, reason: FailureReason) -> bool {
        self.mark_failed_record(id, reason).await.is_some()
    }

    pub async fn mark_failed_with_supervision(
        &self,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<(ActorId, SupervisionDecision)> {
        let supervisor_id = self.mark_failed_record(id, reason.clone()).await?;
        let decision = {
            let mut supervisors = self.supervisors.write().await;
            supervisors
                .entry(supervisor_id.clone())
                .or_default()
                .record_failure(id, reason)
        };

        Some((supervisor_id, decision))
    }

    pub async fn record_actor_error(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        reason: FailureReason,
    ) -> Option<SupervisionDecision> {
        let (supervisor_id, decision) = self
            .mark_failed_with_supervision(actor_id, reason.clone())
            .await?;

        self.emit_lifecycle(
            session_id,
            ActorLifecycleEvent::StatusChanged {
                actor_id: actor_id.clone(),
                status: ActorStatus::Terminated,
                at: Utc::now(),
            },
        );

        match reason {
            FailureReason::Cancelled => self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Cancelled {
                    actor_id: actor_id.clone(),
                    cancelled_at: Utc::now(),
                },
            ),
            reason => self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Failed {
                    actor_id: actor_id.clone(),
                    failed_at: Utc::now(),
                    reason,
                },
            ),
        }

        self.emit_and_apply_supervision_decision(session_id, supervisor_id, decision.clone())
            .await;
        Some(decision)
    }

    pub async fn emit_and_apply_supervision_decision(
        &self,
        session_id: &str,
        supervisor_id: ActorId,
        decision: SupervisionDecision,
    ) {
        if !decision.actions.is_empty() {
            self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Supervision {
                    supervisor_id,
                    failed_actor_id: decision.failed_actor_id.clone(),
                    decision: decision.clone(),
                    decided_at: Utc::now(),
                },
            );
            self.apply_supervision_actions(session_id, &decision).await;
        }
    }

    pub async fn prepare_actor_restart(&self, actor_id: &ActorId) -> Option<ActorSpec> {
        let template = self.restart_templates.read().await.get(actor_id).cloned()?;
        let cancellation = ActorCancelToken::default();
        {
            self.cancel_tokens
                .write()
                .await
                .insert(actor_id.clone(), cancellation.clone());
            self.actor_supervisors
                .write()
                .await
                .insert(actor_id.clone(), template.supervisor_id.clone());
            self.inboxes
                .write()
                .await
                .insert(actor_id.clone(), ActorInbox::new());
        }

        let spawned_at = Utc::now();
        {
            let mut actors = self.actors.write().await;
            let record = actors
                .entry(actor_id.clone())
                .or_insert_with(|| ActorRecord {
                    id: actor_id.clone(),
                    parent: template.parent.clone(),
                    status: ActorStatus::Running,
                    mode: Some(template.role.clone()),
                    budget: template.budget.clone(),
                    spawned_at,
                    completed_at: None,
                    cancelled: false,
                    failure_reason: None,
                    last_summary: None,
                    artifact_uri: None,
                    history_uri: None,
                });
            record.parent = template.parent.clone();
            record.status = ActorStatus::Running;
            record.mode = Some(template.role.clone());
            record.budget = template.budget.clone();
            record.spawned_at = spawned_at;
            record.completed_at = None;
            record.cancelled = false;
            record.failure_reason = None;
            record.last_summary = None;
            record.artifact_uri = None;
            record.history_uri = None;
        }

        self.supervisors
            .write()
            .await
            .entry(template.supervisor_id.clone())
            .or_default()
            .track(actor_id.clone());

        self.emit_lifecycle(
            &template.session_id,
            ActorLifecycleEvent::StatusChanged {
                actor_id: actor_id.clone(),
                status: ActorStatus::Running,
                at: spawned_at,
            },
        );
        self.emit_lifecycle(
            &template.session_id,
            ActorLifecycleEvent::Spawned {
                actor_id: actor_id.clone(),
                parent_id: template.parent.clone(),
                role: template.role.clone(),
                task: template.task.clone(),
                depth: template.depth,
                budget: template.budget.clone(),
                spawned_at,
            },
        );

        Some(template.to_spec(cancellation))
    }

    pub async fn cancel_actor(&self, session_id: &str, id: &ActorId) -> CancelActorResult {
        let cancelled_at = Utc::now();
        let result = {
            let mut actors = self.actors.write().await;
            match actors.get_mut(id) {
                Some(rec) if rec.cancelled => CancelActorResult::AlreadyCancelled,
                Some(rec) if rec.status == ActorStatus::Terminated => {
                    CancelActorResult::AlreadyTerminated
                }
                Some(rec) => {
                    rec.status = ActorStatus::Terminated;
                    rec.completed_at = Some(cancelled_at);
                    rec.cancelled = true;
                    rec.failure_reason = Some(FailureReason::Cancelled);
                    CancelActorResult::Cancelled
                }
                None => CancelActorResult::NotFound,
            }
        };

        if matches!(
            result,
            CancelActorResult::Cancelled | CancelActorResult::AlreadyCancelled
        ) {
            self.cancel_token(id).await.cancel();
        }
        if result == CancelActorResult::Cancelled {
            self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::StatusChanged {
                    actor_id: id.clone(),
                    status: ActorStatus::Terminated,
                    at: cancelled_at,
                },
            );
            self.emit_lifecycle(
                session_id,
                ActorLifecycleEvent::Cancelled {
                    actor_id: id.clone(),
                    cancelled_at,
                },
            );
        }
        result
    }

    pub async fn cancel_actor_subtree(
        &self,
        session_id: &str,
        root: &ActorId,
    ) -> Vec<(ActorId, CancelActorResult)> {
        let actor_ids = self.actor_subtree_ids(root).await;
        let mut results = Vec::with_capacity(actor_ids.len());
        for actor_id in actor_ids {
            let result = self.cancel_actor(session_id, &actor_id).await;
            results.push((actor_id, result));
        }
        results
    }

    pub async fn cancel_token(&self, actor_id: &ActorId) -> ActorCancelToken {
        self.ensure_cancel_token(actor_id).await
    }

    pub async fn is_cancelled(&self, actor_id: &ActorId) -> bool {
        self.cancel_tokens
            .read()
            .await
            .get(actor_id)
            .is_some_and(ActorCancelToken::is_cancelled)
    }

    /// Return true when `waiter` would explicitly wait on itself or a descendant.
    ///
    /// The synthetic top-level Root actor is intentionally exempt: top-level
    /// orchestrator code must be able to await root-level workers. Real actor
    /// descendants are checked through the tracked parent chain.
    pub async fn would_wait_on_descendant(&self, waiter: &ActorId, target: &ActorId) -> bool {
        if waiter.as_str() == "Root" {
            return false;
        }
        if waiter == target {
            return true;
        }
        self.is_descendant_of(target, waiter).await
    }

    /// Return true when `actor_id` has `ancestor_id` in its tracked parent chain.
    pub async fn is_descendant_of(&self, actor_id: &ActorId, ancestor_id: &ActorId) -> bool {
        let actors = self.actors.read().await;
        let mut current = actor_id.clone();
        let mut seen = HashSet::new();

        while seen.insert(current.clone()) {
            let Some(record) = actors.get(&current) else {
                return false;
            };
            let Some(parent) = record.parent.as_ref() else {
                return false;
            };
            if parent == ancestor_id {
                return true;
            }
            current = parent.clone();
        }

        false
    }

    /// Snapshot of all tracked actor records.
    pub async fn list(&self) -> Vec<ActorRecord> {
        self.actors.read().await.values().cloned().collect()
    }

    pub async fn live_direct_children(&self, parent_id: &ActorId) -> Vec<ActorRecord> {
        let actors = self.actors.read().await;
        actors
            .values()
            .filter(|record| Self::is_live_status(record.status))
            .filter(|record| match record.parent.as_ref() {
                Some(parent) => parent == parent_id,
                None => parent_id.as_str() == "Root",
            })
            .cloned()
            .collect()
    }

    pub fn is_live_status(status: ActorStatus) -> bool {
        status != ActorStatus::Terminated
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

    async fn ensure_cancel_token(&self, actor_id: &ActorId) -> ActorCancelToken {
        if let Some(token) = self.cancel_tokens.read().await.get(actor_id).cloned() {
            return token;
        }
        let mut tokens = self.cancel_tokens.write().await;
        tokens.entry(actor_id.clone()).or_default().clone()
    }

    async fn is_reachable(&self, actor_id: &ActorId) -> bool {
        if actor_id.as_str() == "Root" {
            return true;
        }
        self.actors
            .read()
            .await
            .get(actor_id)
            .is_some_and(|record| Self::is_live_status(record.status))
    }

    async fn mark_complete_record(
        &self,
        id: &ActorId,
        digest: Option<(String, Option<String>, Option<String>)>,
    ) -> Option<ActorId> {
        let mut actors = self.actors.write().await;
        let rec = actors.get_mut(id)?;
        if rec.cancelled || rec.failure_reason.is_some() {
            return None;
        }
        rec.status = ActorStatus::Terminated;
        rec.completed_at = Some(Utc::now());
        if let Some((summary, artifact_uri, history_uri)) = digest {
            rec.last_summary = Some(summary);
            rec.artifact_uri = artifact_uri;
            rec.history_uri = history_uri;
        }
        let fallback = rec.parent.clone();
        drop(actors);
        Some(self.supervisor_id_for_actor(id, fallback).await)
    }

    async fn mark_failed_record(&self, id: &ActorId, reason: FailureReason) -> Option<ActorId> {
        let (fallback_supervisor, should_cancel_token) = {
            let mut actors = self.actors.write().await;
            let rec = actors.get_mut(id)?;
            if rec.cancelled {
                return None;
            }
            rec.status = ActorStatus::Terminated;
            rec.completed_at = Some(Utc::now());
            rec.cancelled = matches!(reason, FailureReason::Cancelled);
            rec.failure_reason = Some(reason);
            (rec.parent.clone(), rec.cancelled)
        };
        if should_cancel_token {
            self.cancel_token(id).await.cancel();
        }
        Some(self.supervisor_id_for_actor(id, fallback_supervisor).await)
    }

    async fn record_supervised_success(&self, supervisor_id: &ActorId, id: &ActorId) {
        if let Some(supervisor) = self.supervisors.write().await.get_mut(supervisor_id) {
            supervisor.record_success(id);
        }
    }

    async fn apply_supervision_actions(&self, session_id: &str, decision: &SupervisionDecision) {
        for action in &decision.actions {
            if let SupervisionAction::Cancel { actor_id } = action {
                let _ = self.cancel_actor_subtree(session_id, actor_id).await;
            }
        }
    }

    async fn actor_subtree_ids(&self, root: &ActorId) -> Vec<ActorId> {
        let actors = self.actors.read().await;
        if !actors.contains_key(root) {
            return vec![root.clone()];
        }

        let mut ordered = Vec::new();
        let mut pending = VecDeque::new();
        let mut seen = HashSet::new();
        pending.push_back(root.clone());

        while let Some(current) = pending.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            ordered.push(current.clone());

            let mut children = actors
                .values()
                .filter(|record| record.parent.as_ref() == Some(&current))
                .map(|record| (record.spawned_at, record.id.clone()))
                .collect::<Vec<_>>();
            children.sort_by(|(left_at, left_id), (right_at, right_id)| {
                left_at.cmp(right_at).then_with(|| left_id.cmp(right_id))
            });
            pending.extend(children.into_iter().map(|(_, child)| child));
        }

        ordered
    }

    async fn supervisor_id_for_actor(
        &self,
        actor_id: &ActorId,
        fallback_parent: Option<ActorId>,
    ) -> ActorId {
        self.actor_supervisors
            .read()
            .await
            .get(actor_id)
            .cloned()
            .or(fallback_parent)
            .unwrap_or_else(root_supervisor_actor_id)
    }
}

fn root_supervisor_actor_id() -> ActorId {
    ActorId::new("Root").expect("Root is always valid")
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
    use crate::supervise::RestartStrategy;

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

    fn test_record_with_parent(id: &str, parent: &str) -> ActorRecord {
        ActorRecord {
            parent: Some(ActorId::new(parent).unwrap()),
            ..test_record(id)
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
    async fn lineage_detects_descendant_wait_edges() {
        let registry = MailboxRegistry::new();
        let parent = ActorId::new("Parent").unwrap();
        let child = ActorId::new("Child").unwrap();
        let grandchild = ActorId::new("Grandchild").unwrap();
        let sibling = ActorId::new("Sibling").unwrap();
        let root = ActorId::new("Root").unwrap();

        registry.track(test_record("Parent")).await;
        registry
            .track(test_record_with_parent("Child", "Parent"))
            .await;
        registry
            .track(test_record_with_parent("Grandchild", "Child"))
            .await;
        registry.track(test_record("Sibling")).await;

        assert!(registry.is_descendant_of(&child, &parent).await);
        assert!(registry.is_descendant_of(&grandchild, &parent).await);
        assert!(!registry.is_descendant_of(&sibling, &parent).await);
        assert!(registry.would_wait_on_descendant(&parent, &child).await);
        assert!(
            registry
                .would_wait_on_descendant(&parent, &grandchild)
                .await
        );
        assert!(registry.would_wait_on_descendant(&parent, &parent).await);
        assert!(!registry.would_wait_on_descendant(&child, &parent).await);
        assert!(!registry.would_wait_on_descendant(&root, &child).await);
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

    #[tokio::test]
    async fn mark_failed_with_supervision_returns_default_restart_decision() {
        let registry = MailboxRegistry::new();
        let id = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;

        let (supervisor_id, decision) = registry
            .mark_failed_with_supervision(&id, FailureReason::Timeout)
            .await
            .expect("tracked actor failure has a supervisor decision");

        assert_eq!(supervisor_id, ActorId::new("Root").unwrap());
        assert_eq!(decision.restart_attempt, Some(1));
        assert_eq!(
            decision.actions,
            vec![SupervisionAction::Restart {
                actor_id: id.clone()
            }]
        );
        let rec = registry.get(&id).await.unwrap();
        assert_eq!(rec.status, ActorStatus::Terminated);
        assert!(matches!(rec.failure_reason, Some(FailureReason::Timeout)));
    }

    #[tokio::test]
    async fn one_for_all_supervision_cancels_siblings_and_emits_decision() {
        let registry = MailboxRegistry::new();
        let root = ActorId::new("Root").unwrap();
        registry
            .set_supervision_policy(
                root.clone(),
                SupervisionPolicy {
                    strategy: RestartStrategy::OneForAll,
                    max_restarts: 1,
                },
            )
            .await;
        registry.track(test_record("Alpha")).await;
        registry.track(test_record("Beta")).await;
        registry.track(test_record("Gamma")).await;
        registry
            .track(test_record_with_parent("AlphaChild", "Alpha"))
            .await;
        registry
            .track(test_record_with_parent("GammaChild", "Gamma"))
            .await;

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_for_hook = Arc::clone(&events);
        registry.set_lifecycle_hook(move |_session_id, event| {
            events_for_hook.lock().unwrap().push(event);
        });

        let beta = ActorId::new("Beta").unwrap();
        let (supervisor_id, decision) = registry
            .mark_failed_with_supervision(&beta, FailureReason::Timeout)
            .await
            .expect("tracked actor failure has a supervisor decision");
        registry
            .emit_and_apply_supervision_decision("session-1", supervisor_id, decision.clone())
            .await;

        assert_eq!(
            decision.actions,
            vec![
                SupervisionAction::Cancel {
                    actor_id: ActorId::new("Alpha").unwrap()
                },
                SupervisionAction::Cancel {
                    actor_id: ActorId::new("Gamma").unwrap()
                },
                SupervisionAction::Restart {
                    actor_id: ActorId::new("Alpha").unwrap()
                },
                SupervisionAction::Restart {
                    actor_id: ActorId::new("Beta").unwrap()
                },
                SupervisionAction::Restart {
                    actor_id: ActorId::new("Gamma").unwrap()
                },
            ]
        );
        assert!(
            registry
                .get(&ActorId::new("Alpha").unwrap())
                .await
                .unwrap()
                .cancelled
        );
        assert!(
            registry
                .get(&ActorId::new("AlphaChild").unwrap())
                .await
                .unwrap()
                .cancelled
        );
        assert!(
            registry
                .get(&ActorId::new("Gamma").unwrap())
                .await
                .unwrap()
                .cancelled
        );
        assert!(
            registry
                .get(&ActorId::new("GammaChild").unwrap())
                .await
                .unwrap()
                .cancelled
        );
        assert!(registry.is_cancelled(&ActorId::new("Alpha").unwrap()).await);
        assert!(
            registry
                .is_cancelled(&ActorId::new("AlphaChild").unwrap())
                .await
        );
        assert!(registry.is_cancelled(&ActorId::new("Gamma").unwrap()).await);
        assert!(
            registry
                .is_cancelled(&ActorId::new("GammaChild").unwrap())
                .await
        );

        let events = events.lock().unwrap();
        assert!(matches!(
            events.first(),
            Some(ActorLifecycleEvent::Supervision { .. })
        ));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ActorLifecycleEvent::Cancelled { .. }))
                .count(),
            4
        );
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
    async fn send_message_to_terminated_actor_returns_unreachable() {
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

        assert_eq!(receipt, Receipt::Unreachable);
        assert!(registry.drain_inbox(&to, None).await.is_empty());
    }

    #[tokio::test]
    async fn send_message_to_full_inbox_returns_backpressured_and_drops() {
        let registry = MailboxRegistry::new();
        let to = ActorId::new("Worker").unwrap();
        registry.track(test_record("Worker")).await;

        for index in 0..MAX_INBOX_MESSAGES {
            let receipt = registry
                .send_message(ActorMessage {
                    from: ActorId::new("Root").unwrap(),
                    to: to.clone(),
                    text: format!("queued {index}"),
                    reply_to: None,
                    sent_at: Utc::now(),
                })
                .await;
            assert_eq!(receipt, Receipt::Delivered);
        }

        let receipt = registry
            .send_message(ActorMessage {
                from: ActorId::new("Root").unwrap(),
                to: to.clone(),
                text: "overflow".to_string(),
                reply_to: None,
                sent_at: Utc::now(),
            })
            .await;

        assert_eq!(receipt, Receipt::Backpressured);
        assert_eq!(registry.unread_count(&to).await, MAX_INBOX_MESSAGES);
        assert_eq!(registry.messages().await.len(), MAX_INBOX_MESSAGES);
        let drained = registry.drain_inbox(&to, None).await;
        assert_eq!(drained.len(), MAX_INBOX_MESSAGES);
        assert!(!drained.iter().any(|message| message.text == "overflow"));
    }

    #[tokio::test]
    async fn live_direct_children_excludes_terminated_only() {
        let registry = MailboxRegistry::new();
        let root = ActorId::new("Root").unwrap();
        registry.track(test_record("RunningChild")).await;
        registry
            .track(ActorRecord {
                status: ActorStatus::Idle,
                ..test_record("IdleChild")
            })
            .await;
        registry
            .track(ActorRecord {
                status: ActorStatus::Parked,
                ..test_record("ParkedChild")
            })
            .await;
        registry
            .track(ActorRecord {
                status: ActorStatus::Terminated,
                completed_at: Some(Utc::now()),
                ..test_record("DoneChild")
            })
            .await;
        registry
            .track(test_record_with_parent("NestedChild", "RunningChild"))
            .await;

        let mut ids: Vec<_> = registry
            .live_direct_children(&root)
            .await
            .into_iter()
            .map(|record| record.id)
            .collect();
        ids.sort();

        assert_eq!(
            ids,
            vec![
                ActorId::new("IdleChild").unwrap(),
                ActorId::new("ParkedChild").unwrap(),
                ActorId::new("RunningChild").unwrap(),
            ]
        );
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
