mod cancellation;
mod inbox;
mod lifecycle;
mod messages;
mod restart;
mod roster;
#[cfg(test)]
mod test_api;
mod types;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::RwLock;

use crate::actor::{ActorCancelToken, ActorId, ActorLifecycleEvent, ActorRecord};
use crate::executor::ActorExecutor;
use crate::supervise::Supervisor;

use inbox::ActorInbox;
use messages::MessageLog;
use restart::ActorRestartTemplate;
pub use types::{ActorMessage, CancelActorResult, Receipt};

type RawEventFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type RawEventHook =
    dyn Fn(String, String, serde_json::Value) -> RawEventFuture + Send + Sync + 'static;

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
/// Completed actors retain their output artifact and history URIs for roster inspection.
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
