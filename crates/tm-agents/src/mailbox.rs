mod inbox;
mod restart;
mod types;

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{RwLock, mpsc};

use crate::actor::{
    ActorCancelToken, ActorId, ActorLifecycleEvent, ActorRecord, ActorSpec, ActorStatus,
};
use crate::executor::ActorExecutor;
use crate::supervise::{
    FailureReason, SupervisionAction, SupervisionDecision, SupervisionPolicy, Supervisor,
};

use inbox::ActorInbox;
use restart::ActorRestartTemplate;
pub use types::{ActorMessage, CancelActorResult, Receipt};

type RawEventFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type RawEventHook =
    dyn Fn(String, String, serde_json::Value) -> RawEventFuture + Send + Sync + 'static;

/// Maximum number and aggregate bytes retained in the cross-actor debug log.
/// Older messages are dropped oldest-first when either bound is exceeded.
const MAX_MESSAGES: usize = 1000;
const MAX_MESSAGE_LOG_BYTES: usize = 512 * 1024;
const MAX_INBOX_MESSAGES: usize = 64;
const MAX_INBOX_BYTES: usize = 64 * 1024;
/// Maximum concurrently live actors retained by one session.
pub const MAX_ACTORS_PER_SESSION: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActorKey {
    session_id: String,
    actor_id: ActorId,
}

impl ActorKey {
    fn new(session_id: &str, actor_id: &ActorId) -> Self {
        Self {
            session_id: session_id.to_string(),
            actor_id: actor_id.clone(),
        }
    }
}

struct LoggedMessage {
    session_id: String,
    message: ActorMessage,
    retained_bytes: usize,
}

#[derive(Default)]
struct MessageLog {
    entries: VecDeque<LoggedMessage>,
    retained_bytes: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("session actor limit of {0} reached")]
    SessionActorLimit(usize),
    #[error("actor text invalid: {0}")]
    InvalidText(String),
}

/// Concurrent roster of live actors, plus the injected executor for `agents.run` (P3.1).
///
/// Keeps both a bounded replay/debug message log and live bounded per-actor inbox queues.
/// Completed actors retain their last digest summary so `agents.msg` can still use the
/// P3 compatibility seeded-continuation path when a target is no longer running.
pub struct MailboxRegistry {
    actors: RwLock<HashMap<ActorKey, ActorRecord>>,
    inboxes: RwLock<HashMap<ActorKey, Arc<ActorInbox>>>,
    cancel_tokens: RwLock<HashMap<ActorKey, ActorCancelToken>>,
    executor: std::sync::OnceLock<Arc<dyn ActorExecutor>>,
    next_seq: AtomicU64,
    next_supervisor_seq: AtomicU64,
    messages: RwLock<MessageLog>,
    /// In-memory actor transcripts keyed by actor id (P3.3).
    /// Populated by ChatActorExecutor after each run; served by HistoryResourceHandler.
    transcripts: RwLock<HashMap<ActorKey, String>>,
    restart_templates: RwLock<HashMap<ActorKey, ActorRestartTemplate>>,
    actor_supervisors: RwLock<HashMap<ActorKey, ActorId>>,
    supervisors: RwLock<HashMap<ActorKey, Supervisor>>,
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
            messages: RwLock::new(MessageLog::default()),
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
        mut payload: serde_json::Value,
    ) -> Result<(), String> {
        tm_memory::redact_json_value(&mut payload);
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
    pub async fn track_for_session(
        &self,
        session_id: &str,
        record: ActorRecord,
    ) -> Result<(), RegistryError> {
        let supervisor_id = record
            .parent
            .clone()
            .unwrap_or_else(root_supervisor_actor_id);
        self.track_with_supervisor_for_session(session_id, record, supervisor_id)
            .await
    }

    pub async fn track_with_supervisor_for_session(
        &self,
        session_id: &str,
        record: ActorRecord,
        supervisor_id: ActorId,
    ) -> Result<(), RegistryError> {
        self.track_batch_for_session(session_id, vec![(record, supervisor_id)])
            .await
    }

    /// Atomically reserve and register a wave of actors for one exact session.
    pub async fn track_batch_for_session(
        &self,
        session_id: &str,
        records: Vec<(ActorRecord, ActorId)>,
    ) -> Result<(), RegistryError> {
        for (record, _) in &records {
            if let Some(mode) = record.mode.as_deref() {
                crate::actor::validate_text_bytes("mode", mode, crate::actor::MAX_ACTOR_ROLE_BYTES)
                    .map_err(RegistryError::InvalidText)?;
            }
        }
        // Acquire every affected shard before mutating any of them. Cancellation while waiting for
        // locks leaves no half-registered wave; after the last await, commit is synchronous.
        let mut actors = self.actors.write().await;
        let mut inboxes = self.inboxes.write().await;
        let mut cancel_tokens = self.cancel_tokens.write().await;
        let mut actor_supervisors = self.actor_supervisors.write().await;
        let mut supervisors = self.supervisors.write().await;
        let live_count = actors
            .iter()
            .filter(|(candidate, record)| {
                candidate.session_id == session_id && Self::is_live_status(record.status)
            })
            .count();
        let new_live_count = records
            .iter()
            .filter(|(record, _)| Self::is_live_status(record.status))
            .filter(|(record, _)| !actors.contains_key(&ActorKey::new(session_id, &record.id)))
            .count();
        if live_count.saturating_add(new_live_count) > MAX_ACTORS_PER_SESSION {
            return Err(RegistryError::SessionActorLimit(MAX_ACTORS_PER_SESSION));
        }
        for (record, supervisor_id) in records {
            let actor_id = record.id.clone();
            let key = ActorKey::new(session_id, &actor_id);
            actors.insert(key.clone(), record);
            inboxes.entry(key.clone()).or_insert_with(ActorInbox::new);
            cancel_tokens.entry(key.clone()).or_default();
            actor_supervisors.insert(key, supervisor_id.clone());
            supervisors
                .entry(ActorKey::new(session_id, &supervisor_id))
                .or_default()
                .track(actor_id);
        }
        Ok(())
    }

    pub async fn remember_restart_spec(&self, spec: &ActorSpec) {
        let supervisor_id = self
            .supervisor_id_for_actor(&spec.session_id, &spec.id, spec.parent.clone())
            .await;
        self.restart_templates.write().await.insert(
            ActorKey::new(&spec.session_id, &spec.id),
            ActorRestartTemplate::from_spec(spec, supervisor_id),
        );
    }

    /// Look up a single actor record by id.
    pub async fn get_for_session(&self, session_id: &str, id: &ActorId) -> Option<ActorRecord> {
        self.actors
            .read()
            .await
            .get(&ActorKey::new(session_id, id))
            .cloned()
    }

    /// Update an actor's status in place.
    pub async fn update_status_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        status: ActorStatus,
    ) {
        if let Some(rec) = self
            .actors
            .write()
            .await
            .get_mut(&ActorKey::new(session_id, id))
        {
            rec.status = status;
        }
    }

    pub async fn set_supervision_policy_for_session(
        &self,
        session_id: &str,
        supervisor_id: ActorId,
        policy: SupervisionPolicy,
    ) {
        let mut supervisors = self.supervisors.write().await;
        let supervisor = supervisors
            .entry(ActorKey::new(session_id, &supervisor_id))
            .or_insert_with(|| Supervisor::new(policy.clone()));
        supervisor.policy = policy;
    }

    /// Mark an actor as successfully terminated.
    pub async fn mark_complete_for_session(&self, session_id: &str, id: &ActorId) -> bool {
        let Some(supervisor_id) = self.mark_complete_record(session_id, id, None).await else {
            return false;
        };
        self.record_supervised_success(session_id, &supervisor_id, id)
            .await;
        true
    }

    /// Mark an actor as successfully terminated and store its full digest (P3.3).
    ///
    /// Stores summary (seeds `agents.msg` continuations), artifact URI, and history URI.
    pub async fn mark_complete_with_digest_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        summary: String,
        artifact_uri: Option<String>,
        history_uri: Option<String>,
    ) -> bool {
        let summary = tm_memory::redact_dream_text(&summary).text;
        let Some(supervisor_id) = self
            .mark_complete_record(session_id, id, Some((summary, artifact_uri, history_uri)))
            .await
        else {
            return false;
        };
        self.record_supervised_success(session_id, &supervisor_id, id)
            .await;
        true
    }

    /// Store the full transcript for an actor (P3.3).
    ///
    /// Called by the orchestrator after `run_to_digest` returns `history_content`.
    /// Served by `HistoryResourceHandler`.
    pub async fn store_transcript_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        content: String,
    ) {
        self.transcripts.write().await.insert(
            ActorKey::new(session_id, id),
            tm_memory::redact_dream_text(&content).text,
        );
    }

    /// Retrieve the stored transcript for an actor, if any.
    pub async fn get_transcript_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
    ) -> Option<String> {
        self.transcripts
            .read()
            .await
            .get(&ActorKey::new(session_id, id))
            .cloned()
    }

    /// Append a message to the bounded in-memory log.
    ///
    /// Oldest messages are dropped once the log exceeds [`MAX_MESSAGES`].
    pub async fn record_message_for_session(
        &self,
        session_id: &str,
        msg: ActorMessage,
    ) -> Result<(), RegistryError> {
        msg.validate().map_err(RegistryError::InvalidText)?;
        let retained_bytes = msg.retained_bytes().saturating_add(session_id.len());
        let mut log = self.messages.write().await;
        while !log.entries.is_empty()
            && (log.entries.len() >= MAX_MESSAGES
                || log.retained_bytes.saturating_add(retained_bytes) > MAX_MESSAGE_LOG_BYTES)
        {
            if let Some(oldest) = log.entries.pop_front() {
                log.retained_bytes = log.retained_bytes.saturating_sub(oldest.retained_bytes);
            }
        }
        if retained_bytes > MAX_MESSAGE_LOG_BYTES {
            return Err(RegistryError::InvalidText(
                "message exceeds aggregate log budget".to_string(),
            ));
        }
        log.retained_bytes = log.retained_bytes.saturating_add(retained_bytes);
        log.entries.push_back(LoggedMessage {
            session_id: session_id.to_string(),
            message: msg,
            retained_bytes,
        });
        Ok(())
    }

    /// Deliver a message to the recipient's live inbox and append it to the bounded log.
    ///
    /// Unknown or terminated actors return [`Receipt::Unreachable`]. Full inboxes return
    /// [`Receipt::Backpressured`] and drop the message. The synthetic `Root`
    /// actor is always reachable so child actors can reply to the top-level orchestrator.
    pub async fn send_message_for_session(
        &self,
        session_id: &str,
        msg: ActorMessage,
    ) -> Result<Receipt, RegistryError> {
        msg.validate().map_err(RegistryError::InvalidText)?;
        if !self.is_reachable(session_id, &msg.to).await {
            return Ok(Receipt::Unreachable);
        }

        let inbox = self.ensure_inbox(session_id, &msg.to).await;
        let retained_bytes = msg.retained_bytes();
        if !inbox.try_reserve(retained_bytes) {
            return Ok(Receipt::Backpressured);
        }
        match inbox.sender.try_send(msg.clone()) {
            Ok(()) => {
                inbox.mark_delivered_to_inbox();
                inbox.touch(msg.sent_at).await;
                if let Err(error) = self.record_message_for_session(session_id, msg).await {
                    tracing::warn!(%error, "message delivered but debug log rejected it");
                }
                Ok(Receipt::Delivered)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                inbox.release(retained_bytes);
                Ok(Receipt::Backpressured)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                inbox.release(retained_bytes);
                Ok(Receipt::Unreachable)
            }
        }
    }

    /// Drain all pending messages for `actor_id`, optionally filtering by sender.
    pub async fn drain_inbox_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        from: Option<&ActorId>,
    ) -> Vec<ActorMessage> {
        let inbox = self.ensure_inbox(session_id, actor_id).await;
        inbox.drain(from).await
    }

    /// Wait for the next matching message for `actor_id` until `timeout` elapses.
    pub async fn wait_for_message_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        from: Option<&ActorId>,
        timeout: Duration,
    ) -> Option<ActorMessage> {
        let inbox = self.ensure_inbox(session_id, actor_id).await;
        inbox.wait(from, timeout).await
    }

    pub async fn unread_count_for_session(&self, session_id: &str, actor_id: &ActorId) -> usize {
        let Some(inbox) = self
            .inboxes
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .cloned()
        else {
            return 0;
        };
        inbox.unread()
    }

    pub async fn last_activity_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
    ) -> Option<DateTime<Utc>> {
        let inbox = self
            .inboxes
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .cloned()?;
        inbox.last_activity().await
    }

    /// Snapshot of all messages in the log (oldest-first).
    pub async fn messages_for_session(&self, session_id: &str) -> Vec<ActorMessage> {
        self.messages
            .read()
            .await
            .entries
            .iter()
            .filter(|logged| logged.session_id == session_id)
            .map(|logged| logged.message.clone())
            .collect()
    }

    /// Mark an actor as terminated due to a failure.
    pub async fn mark_failed_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        reason: FailureReason,
    ) -> bool {
        self.mark_failed_record(session_id, id, reason)
            .await
            .is_some()
    }

    pub async fn mark_failed_with_supervision_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<(ActorId, SupervisionDecision)> {
        let supervisor_id = self
            .mark_failed_record(session_id, id, reason.clone())
            .await?;
        let decision = {
            let mut supervisors = self.supervisors.write().await;
            supervisors
                .entry(ActorKey::new(session_id, &supervisor_id))
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
            .mark_failed_with_supervision_for_session(session_id, actor_id, reason.clone())
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

    pub async fn prepare_actor_restart_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
    ) -> Option<ActorSpec> {
        let key = ActorKey::new(session_id, actor_id);
        let template = self.restart_templates.read().await.get(&key).cloned()?;
        let cancellation = ActorCancelToken::default();
        {
            self.cancel_tokens
                .write()
                .await
                .insert(key.clone(), cancellation.clone());
            self.actor_supervisors
                .write()
                .await
                .insert(key.clone(), template.supervisor_id.clone());
            self.inboxes
                .write()
                .await
                .insert(key.clone(), ActorInbox::new());
        }

        let spawned_at = Utc::now();
        {
            let mut actors = self.actors.write().await;
            let record = actors.entry(key).or_insert_with(|| ActorRecord {
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
            .entry(ActorKey::new(session_id, &template.supervisor_id))
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
            match actors.get_mut(&ActorKey::new(session_id, id)) {
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
            self.cancel_token_for_session(session_id, id).await.cancel();
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
        let actor_ids = self.actor_subtree_ids(session_id, root).await;
        let mut results = Vec::with_capacity(actor_ids.len());
        for actor_id in actor_ids {
            let result = self.cancel_actor(session_id, &actor_id).await;
            results.push((actor_id, result));
        }
        results
    }

    pub async fn cancel_token_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
    ) -> ActorCancelToken {
        self.ensure_cancel_token(session_id, actor_id).await
    }

    pub async fn is_cancelled_for_session(&self, session_id: &str, actor_id: &ActorId) -> bool {
        self.cancel_tokens
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .is_some_and(ActorCancelToken::is_cancelled)
    }

    /// Return true when `waiter` would explicitly wait on itself or a descendant.
    ///
    /// The synthetic top-level Root actor is intentionally exempt: top-level
    /// orchestrator code must be able to await root-level workers. Real actor
    /// descendants are checked through the tracked parent chain.
    pub async fn would_wait_on_descendant_for_session(
        &self,
        session_id: &str,
        waiter: &ActorId,
        target: &ActorId,
    ) -> bool {
        if waiter.as_str() == "Root" {
            return false;
        }
        if waiter == target {
            return true;
        }
        self.is_descendant_of_for_session(session_id, target, waiter)
            .await
    }

    /// Return true when `actor_id` has `ancestor_id` in its tracked parent chain.
    pub async fn is_descendant_of_for_session(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        ancestor_id: &ActorId,
    ) -> bool {
        let actors = self.actors.read().await;
        let mut current = actor_id.clone();
        let mut seen = HashSet::new();

        while seen.insert(current.clone()) {
            let Some(record) = actors.get(&ActorKey::new(session_id, &current)) else {
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
    pub async fn list_for_session(&self, session_id: &str) -> Vec<ActorRecord> {
        self.actors
            .read()
            .await
            .iter()
            .filter(|(key, _)| key.session_id == session_id)
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub async fn live_direct_children_for_session(
        &self,
        session_id: &str,
        parent_id: &ActorId,
    ) -> Vec<ActorRecord> {
        let actors = self.actors.read().await;
        actors
            .iter()
            .filter(|(key, _)| key.session_id == session_id)
            .map(|(_, record)| record)
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

    async fn ensure_inbox(&self, session_id: &str, actor_id: &ActorId) -> Arc<ActorInbox> {
        let key = ActorKey::new(session_id, actor_id);
        if let Some(inbox) = self.inboxes.read().await.get(&key).cloned() {
            return inbox;
        }
        let mut inboxes = self.inboxes.write().await;
        inboxes.entry(key).or_insert_with(ActorInbox::new).clone()
    }

    async fn ensure_cancel_token(&self, session_id: &str, actor_id: &ActorId) -> ActorCancelToken {
        let key = ActorKey::new(session_id, actor_id);
        if let Some(token) = self.cancel_tokens.read().await.get(&key).cloned() {
            return token;
        }
        let mut tokens = self.cancel_tokens.write().await;
        tokens.entry(key).or_default().clone()
    }

    async fn is_reachable(&self, session_id: &str, actor_id: &ActorId) -> bool {
        if actor_id.as_str() == "Root" {
            return true;
        }
        self.actors
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .is_some_and(|record| Self::is_live_status(record.status))
    }

    async fn mark_complete_record(
        &self,
        session_id: &str,
        id: &ActorId,
        digest: Option<(String, Option<String>, Option<String>)>,
    ) -> Option<ActorId> {
        let mut actors = self.actors.write().await;
        let rec = actors.get_mut(&ActorKey::new(session_id, id))?;
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
        Some(self.supervisor_id_for_actor(session_id, id, fallback).await)
    }

    async fn mark_failed_record(
        &self,
        session_id: &str,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<ActorId> {
        let (fallback_supervisor, should_cancel_token) = {
            let mut actors = self.actors.write().await;
            let rec = actors.get_mut(&ActorKey::new(session_id, id))?;
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
            self.cancel_token_for_session(session_id, id).await.cancel();
        }
        Some(
            self.supervisor_id_for_actor(session_id, id, fallback_supervisor)
                .await,
        )
    }

    async fn record_supervised_success(
        &self,
        session_id: &str,
        supervisor_id: &ActorId,
        id: &ActorId,
    ) {
        if let Some(supervisor) = self
            .supervisors
            .write()
            .await
            .get_mut(&ActorKey::new(session_id, supervisor_id))
        {
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

    async fn actor_subtree_ids(&self, session_id: &str, root: &ActorId) -> Vec<ActorId> {
        let actors = self.actors.read().await;
        if !actors.contains_key(&ActorKey::new(session_id, root)) {
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
                .iter()
                .filter(|(key, _)| key.session_id == session_id)
                .map(|(_, record)| record)
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
        session_id: &str,
        actor_id: &ActorId,
        fallback_parent: Option<ActorId>,
    ) -> ActorId {
        self.actor_supervisors
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .cloned()
            .or(fallback_parent)
            .unwrap_or_else(root_supervisor_actor_id)
    }

    // Unit tests written before session ownership use one isolated synthetic session. Keep these
    // compatibility helpers test-only so production callers must always supply exact ownership.
    #[cfg(test)]
    pub async fn track(&self, record: ActorRecord) {
        self.track_for_session("", record).await.unwrap();
    }

    #[cfg(test)]
    pub async fn track_with_supervisor(&self, record: ActorRecord, supervisor_id: ActorId) {
        self.track_with_supervisor_for_session("", record, supervisor_id)
            .await
            .unwrap();
    }

    #[cfg(test)]
    pub async fn get(&self, id: &ActorId) -> Option<ActorRecord> {
        self.get_for_session("", id).await
    }

    #[cfg(test)]
    pub async fn update_status(&self, id: &ActorId, status: ActorStatus) {
        self.update_status_for_session("", id, status).await;
    }

    #[cfg(test)]
    pub async fn set_supervision_policy(&self, supervisor_id: ActorId, policy: SupervisionPolicy) {
        self.set_supervision_policy_for_session("", supervisor_id, policy)
            .await;
    }

    #[cfg(test)]
    pub async fn mark_complete(&self, id: &ActorId) -> bool {
        self.mark_complete_for_session("", id).await
    }

    #[cfg(test)]
    pub async fn mark_complete_with_digest(
        &self,
        id: &ActorId,
        summary: String,
        artifact_uri: Option<String>,
        history_uri: Option<String>,
    ) -> bool {
        self.mark_complete_with_digest_for_session("", id, summary, artifact_uri, history_uri)
            .await
    }

    #[cfg(test)]
    pub async fn store_transcript(&self, id: &ActorId, content: String) {
        self.store_transcript_for_session("", id, content).await;
    }

    #[cfg(test)]
    pub async fn get_transcript(&self, id: &ActorId) -> Option<String> {
        self.get_transcript_for_session("", id).await
    }

    #[cfg(test)]
    pub async fn record_message(&self, message: ActorMessage) {
        self.record_message_for_session("", message).await.unwrap();
    }

    #[cfg(test)]
    pub async fn send_message(&self, message: ActorMessage) -> Receipt {
        self.send_message_for_session("", message).await.unwrap()
    }

    #[cfg(test)]
    pub async fn drain_inbox(
        &self,
        actor_id: &ActorId,
        from: Option<&ActorId>,
    ) -> Vec<ActorMessage> {
        self.drain_inbox_for_session("", actor_id, from).await
    }

    #[cfg(test)]
    pub async fn wait_for_message(
        &self,
        actor_id: &ActorId,
        from: Option<&ActorId>,
        timeout: Duration,
    ) -> Option<ActorMessage> {
        self.wait_for_message_for_session("", actor_id, from, timeout)
            .await
    }

    #[cfg(test)]
    pub async fn unread_count(&self, actor_id: &ActorId) -> usize {
        self.unread_count_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn last_activity(&self, actor_id: &ActorId) -> Option<DateTime<Utc>> {
        self.last_activity_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn messages(&self) -> Vec<ActorMessage> {
        self.messages_for_session("").await
    }

    #[cfg(test)]
    pub async fn mark_failed(&self, id: &ActorId, reason: FailureReason) -> bool {
        self.mark_failed_for_session("", id, reason).await
    }

    #[cfg(test)]
    pub async fn mark_failed_with_supervision(
        &self,
        id: &ActorId,
        reason: FailureReason,
    ) -> Option<(ActorId, SupervisionDecision)> {
        self.mark_failed_with_supervision_for_session("", id, reason)
            .await
    }

    #[cfg(test)]
    pub async fn prepare_actor_restart(&self, actor_id: &ActorId) -> Option<ActorSpec> {
        self.prepare_actor_restart_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn cancel_token(&self, actor_id: &ActorId) -> ActorCancelToken {
        self.cancel_token_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn is_cancelled(&self, actor_id: &ActorId) -> bool {
        self.is_cancelled_for_session("", actor_id).await
    }

    #[cfg(test)]
    pub async fn would_wait_on_descendant(&self, waiter: &ActorId, target: &ActorId) -> bool {
        self.would_wait_on_descendant_for_session("", waiter, target)
            .await
    }

    #[cfg(test)]
    pub async fn is_descendant_of(&self, actor_id: &ActorId, ancestor_id: &ActorId) -> bool {
        self.is_descendant_of_for_session("", actor_id, ancestor_id)
            .await
    }

    #[cfg(test)]
    pub async fn list(&self) -> Vec<ActorRecord> {
        self.list_for_session("").await
    }

    #[cfg(test)]
    pub async fn live_direct_children(&self, parent_id: &ActorId) -> Vec<ActorRecord> {
        self.live_direct_children_for_session("", parent_id).await
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
mod tests;
