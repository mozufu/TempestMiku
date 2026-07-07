use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use serde_json::json;
use tm_artifacts::ArtifactStore;
use tm_core::{
    Agent, AgentConfig, CancellationToken, EventSink, InboxDrain, LlmClient, Sandbox, ToolMediator,
};
use tm_host::CapabilityGrants;
use tm_modes::ModeId;
use tm_sandbox::{DenoSandbox, DenoSandboxOptions};
use uuid::Uuid;

use tm_agents::{ActorDigest, ActorError, ActorExecutor, ActorSpec};

use crate::{CodingEventSink, Result, ServerError, SessionEvent, Store, StoreEvent};

#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub session_id: Uuid,
    pub user_prompt: String,
    pub mode: ModeId,
    pub scope: String,
    pub system_prompt: String,
}

#[async_trait]
pub trait ChatRunner: Send + Sync + 'static {
    /// `mediator`, when present, lets the model propose a mid-turn action (e.g. a mode
    /// switch) through a second tool alongside `execute`; see [`tm_core::ToolMediator`].
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
        mediator: Option<Arc<dyn ToolMediator>>,
    ) -> Result<String>;
}

#[async_trait]
impl<T: ChatRunner + ?Sized> ChatRunner for Arc<T> {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
        mediator: Option<Arc<dyn ToolMediator>>,
    ) -> Result<String> {
        (**self).run_turn(turn, sink, mediator).await
    }
}

struct AgentRequest {
    turn: ChatTurn,
    sink: Arc<dyn EventSink + Send + Sync>,
    mediator: Option<Arc<dyn ToolMediator>>,
    reply: tokio::sync::oneshot::Sender<Result<String>>,
}

pub struct AgentChatRunner {
    sender: Mutex<std::sync::mpsc::Sender<AgentRequest>>,
}

impl AgentChatRunner {
    pub fn new(agent: Agent) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<AgentRequest>();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent worker runtime builds");
            for request in receiver {
                let result = runtime
                    .block_on(agent.run_with_inbox(
                        &request.turn.user_prompt,
                        request.sink.as_ref(),
                        None,
                        request.mediator.as_deref(),
                    ))
                    .map_err(|err| ServerError::Store(err.to_string()));
                let _ = request.reply.send(result);
            }
        });
        Self {
            sender: Mutex::new(sender),
        }
    }

    pub fn deno(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        base_options: DenoSandboxOptions,
    ) -> Self {
        Self::new_with_sandbox_factory(llm, cfg, move |session_id| {
            let mut options = base_options.clone();
            options.session_id = session_id.to_string();
            Arc::new(DenoSandbox::new(options))
        })
    }

    pub fn new_with_sandbox_factory(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        let sandbox_factory = Arc::new(sandbox_factory);
        let (sender, receiver) = std::sync::mpsc::channel::<AgentRequest>();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent worker runtime builds");
            for request in receiver {
                let sandbox = sandbox_factory(request.turn.session_id);
                let mut cfg = cfg.clone();
                cfg.system_prompt = request.turn.system_prompt.clone();
                let agent = Agent::new(Arc::clone(&llm), sandbox, cfg);
                let result = runtime
                    .block_on(agent.run_with_inbox(
                        &request.turn.user_prompt,
                        request.sink.as_ref(),
                        None,
                        request.mediator.as_deref(),
                    ))
                    .map_err(|err| ServerError::Store(err.to_string()));
                let _ = request.reply.send(result);
            }
        });
        Self {
            sender: Mutex::new(sender),
        }
    }
}

impl ChatActorExecutor {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        Self::with_artifact_root(llm, cfg, sandbox_factory, None)
    }

    pub fn with_artifact_root(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync + 'static,
        artifact_root: Option<PathBuf>,
    ) -> Self {
        Self {
            llm,
            cfg,
            sandbox_factory: Arc::new(move |session_id, _actor_id, _grants, _cancellation| {
                sandbox_factory(session_id)
            }),
            artifact_root,
            actor_roster: None,
        }
    }

    pub fn with_actor_context(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(
            Uuid,
            Option<&str>,
            &CapabilityGrants,
            Option<Arc<dyn CancellationToken>>,
        ) -> Arc<dyn Sandbox>
        + Send
        + Sync
        + 'static,
        artifact_root: Option<PathBuf>,
        actor_roster: Arc<tm_agents::MailboxRegistry>,
    ) -> Self {
        Self {
            llm,
            cfg,
            sandbox_factory: Arc::new(sandbox_factory),
            artifact_root,
            actor_roster: Some(actor_roster),
        }
    }
}

#[async_trait]
impl ChatRunner for AgentChatRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
        mediator: Option<Arc<dyn ToolMediator>>,
    ) -> Result<String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| ServerError::Store("agent sender lock poisoned".to_string()))?;
            sender
                .send(AgentRequest {
                    turn,
                    sink,
                    mediator,
                    reply,
                })
                .map_err(|_| ServerError::Store("agent worker stopped".to_string()))?;
        }
        response
            .await
            .map_err(|_| ServerError::Store("agent worker dropped response".to_string()))?
    }
}

#[derive(Debug, Default)]
pub struct EchoChatRunner;

#[async_trait]
impl ChatRunner for EchoChatRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
        _mediator: Option<Arc<dyn ToolMediator>>,
    ) -> Result<String> {
        let text = format!("Miku heard: {}", turn.user_prompt);
        sink.on_text(&text);
        sink.on_final(&text);
        Ok(text)
    }
}

/// Concrete dispatch enum so `main.rs` can pick a runner at startup without making
/// `AppState` generic over a trait object (which loses `Sized`).
pub enum ServerChatRunner {
    Echo(EchoChatRunner),
    Agent(AgentChatRunner),
}

#[async_trait]
impl ChatRunner for ServerChatRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
        mediator: Option<Arc<dyn ToolMediator>>,
    ) -> Result<String> {
        match self {
            Self::Echo(r) => r.run_turn(turn, sink, mediator).await,
            Self::Agent(r) => r.run_turn(turn, sink, mediator).await,
        }
    }
}

pub struct PersistingEventSink<S> {
    session_id: Uuid,
    store: Arc<S>,
    sender: tokio::sync::broadcast::Sender<SessionEvent>,
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl<S> PersistingEventSink<S> {
    pub fn new(
        session_id: Uuid,
        store: Arc<S>,
        sender: tokio::sync::broadcast::Sender<SessionEvent>,
    ) -> Self {
        Self {
            session_id,
            store,
            sender,
            events: Mutex::new(Vec::new()),
        }
    }

    fn push_event(&self, event_type: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .expect("event buffer lock poisoned")
            .push((event_type.to_string(), payload));
    }
}

impl<S> EventSink for PersistingEventSink<S>
where
    S: Store,
{
    fn on_text(&self, delta: &str) {
        self.push_event(
            "text",
            serde_json::to_value(StoreEvent::Text {
                delta: delta.to_string(),
            })
            .expect("text event serializes"),
        );
    }

    fn on_tool_call(&self, name: &str) {
        self.push_event("tool_call", json!({ "name": name }));
    }

    fn on_cell_start(&self, code: &str) {
        self.push_event("cell_start", json!({ "code": code }));
    }

    fn on_cell_result(&self, shaped: &str) {
        self.push_event("cell_result", json!({ "shaped": shaped }));
    }

    fn on_final(&self, text: &str) {
        self.push_event(
            "final",
            serde_json::to_value(StoreEvent::Final {
                text: text.to_string(),
            })
            .expect("final event serializes"),
        );
    }
}

impl<S> PersistingEventSink<S>
where
    S: Store,
{
    pub async fn flush(&self) -> Result<()> {
        let pending = {
            let mut events = self.events.lock().expect("event buffer lock poisoned");
            std::mem::take(&mut *events)
        };
        for (event_type, payload) in pending {
            let event = self
                .store
                .append_event(self.session_id, &event_type, payload)
                .await?;
            let _ = self.sender.send(event);
        }
        Ok(())
    }
}

// ─── RosterCodingEventSink ───────────────────────────────────────────────────

/// Persists coding-style events through the actor roster's raw event hook.
///
/// This is used by child actor approval policies. Child actors run on their own
/// worker threads, but their user-visible approval prompts still belong to the
/// parent session event log.
pub struct RosterCodingEventSink {
    session_id: Uuid,
    roster: Arc<tm_agents::MailboxRegistry>,
}

impl RosterCodingEventSink {
    pub fn new(session_id: Uuid, roster: Arc<tm_agents::MailboxRegistry>) -> Self {
        Self { session_id, roster }
    }
}

#[async_trait]
impl CodingEventSink for RosterCodingEventSink {
    async fn emit(
        &self,
        event_type: &str,
        payload_json: serde_json::Value,
    ) -> Result<SessionEvent> {
        self.roster
            .emit_raw_event(
                &self.session_id.to_string(),
                event_type,
                payload_json.clone(),
            )
            .await
            .map_err(ServerError::Store)?;
        Ok(SessionEvent::new(
            self.session_id,
            0,
            event_type,
            payload_json,
            chrono::Utc::now(),
        ))
    }
}

// ─── CollectingSink ───────────────────────────────────────────────────────────

/// Captures all agent events as a plain-text transcript (P3.3).
///
/// Used by `ChatActorExecutor` to record sub-agent output for `history://` resources.
struct CollectingSink(Mutex<Vec<String>>);

impl CollectingSink {
    fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn push(&self, line: String) {
        self.0.lock().expect("collecting sink lock").push(line);
    }

    fn into_transcript(self) -> String {
        self.0
            .into_inner()
            .expect("collecting sink lock")
            .join("\n")
    }
}

impl EventSink for CollectingSink {
    fn on_text(&self, delta: &str) {
        self.push(format!("[text] {delta}"));
    }
    fn on_tool_call(&self, name: &str) {
        self.push(format!("[tool_call] {name}"));
    }
    fn on_cell_start(&self, code: &str) {
        self.push(format!("[cell_start] {code}"));
    }
    fn on_cell_result(&self, shaped: &str) {
        self.push(format!("[cell_result] {shaped}"));
    }
    fn on_final(&self, text: &str) {
        self.push(format!("[final] {text}"));
    }
}

// ─── ChatActorExecutor ────────────────────────────────────────────────────────

/// Runs sub-agents using the existing [`Agent`] loop without routing through
/// [`AgentChatRunner`]'s blocking channel (which would deadlock when called from
/// inside a parent actor's sandbox cell).
///
/// Injected into [`tm_agents::MailboxRegistry`] at startup; called by the `agents.run`
/// HostFn body.
pub struct ChatActorExecutor {
    llm: Arc<dyn LlmClient>,
    cfg: AgentConfig,
    sandbox_factory: Arc<
        dyn Fn(
                Uuid,
                Option<&str>,
                &CapabilityGrants,
                Option<Arc<dyn CancellationToken>>,
            ) -> Arc<dyn Sandbox>
            + Send
            + Sync,
    >,
    /// Artifact root for reading child cell-spills and writing transcripts (P3.3).
    artifact_root: Option<PathBuf>,
    actor_roster: Option<Arc<tm_agents::MailboxRegistry>>,
}

struct ActorMailboxDrain {
    roster: Arc<tm_agents::MailboxRegistry>,
    actor_id: tm_agents::ActorId,
}

#[async_trait]
impl InboxDrain for ActorMailboxDrain {
    async fn drain(&self) -> tm_core::Result<Vec<String>> {
        Ok(self
            .roster
            .drain_inbox(&self.actor_id, None)
            .await
            .into_iter()
            .map(|message| {
                let reply_to = message
                    .reply_to
                    .as_ref()
                    .map(|id| format!("\nreplyTo: {}", id.as_str()))
                    .unwrap_or_default();
                format!(
                    "from: {}\nsentAt: {}\ntext: {}{}",
                    message.from.as_str(),
                    message.sent_at.to_rfc3339(),
                    message.text,
                    reply_to
                )
            })
            .collect())
    }
}

#[async_trait]
impl ActorExecutor for ChatActorExecutor {
    async fn run_to_digest(&self, spec: ActorSpec) -> std::result::Result<ActorDigest, ActorError> {
        if spec.depth >= spec.budget.max_depth {
            return Err(ActorError::DepthExceeded(spec.depth));
        }
        if spec.cancellation.is_cancelled() {
            return Err(ActorError::Cancelled);
        }

        // DenoSandbox sessions are !Send (Deno JS runtime is single-threaded), so we
        // cannot await agent.run() directly in an async_trait Send future. Spawn a
        // dedicated thread with its own single-threaded tokio runtime — the same
        // isolation pattern as AgentChatRunner, but one thread per actor call rather
        // than a shared sequential queue (which would deadlock when called from inside
        // a parent actor's sandbox cell).
        let llm = Arc::clone(&self.llm);
        let mut cfg = self.cfg.clone();
        cfg.system_prompt = format!(
            "You are a specialized sub-agent. Role: {}.\n\
             Complete the assigned task. When finished, provide a plain-prose summary of your result.",
            spec.role,
        );
        let task = spec.task.clone();
        let actor_id = spec.id.clone();
        let cancellation: Arc<dyn CancellationToken> = Arc::new(spec.cancellation.clone());
        let artifact_root = self.artifact_root.clone();
        let owner_session_id = spec
            .session_id
            .parse::<Uuid>()
            .unwrap_or_else(|_| Uuid::new_v4());
        let existing_artifact_ids = artifact_root
            .as_ref()
            .and_then(|root| ArtifactStore::open(root, owner_session_id.to_string()).ok())
            .map(|store| {
                store
                    .list()
                    .into_iter()
                    .map(|artifact| artifact.id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let sandbox = (self.sandbox_factory)(
            owner_session_id,
            Some(actor_id.as_str()),
            &spec.grants,
            Some(Arc::clone(&cancellation)),
        );
        let inbox = self.actor_roster.as_ref().map(|roster| ActorMailboxDrain {
            roster: Arc::clone(roster),
            actor_id: actor_id.clone(),
        });
        let cancellation_for_loop = Arc::clone(&cancellation);

        let (tx, rx) =
            tokio::sync::oneshot::channel::<std::result::Result<(String, String), ActorError>>();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("actor worker runtime builds");
            let agent = Agent::new(llm, sandbox, cfg);
            let sink = CollectingSink::new();
            let result = runtime
                .block_on(agent.run_with_controls(
                    &task,
                    &sink,
                    inbox.as_ref().map(|inbox| inbox as &dyn InboxDrain),
                    // Sub-agents run scoped tasks, not the top-level conversation; they
                    // don't get a mode-suggest (or any other) mediator.
                    None,
                    Some(cancellation_for_loop.as_ref()),
                ))
                .map_err(|e| match e {
                    tm_core::Error::Cancelled => ActorError::Cancelled,
                    other => ActorError::Execution(other.to_string()),
                });
            let transcript = sink.into_transcript();
            let _ = tx.send(result.map(|summary| (summary, transcript)));
        });

        let (summary, transcript) = rx
            .await
            .map_err(|_| ActorError::Execution("actor worker dropped response".to_string()))??;

        // Populate artifact_uri from this child's own transcript first; concurrent child actors
        // can write to the same session artifact store, so "latest new artifact" is only a fallback.
        let (artifact_uri, history_uri) = if let Some(ref root) = artifact_root {
            match ArtifactStore::open(root, owner_session_id.to_string()) {
                Ok(store) => {
                    let artifact_uri = last_artifact_uri_in_text(&transcript).or_else(|| {
                        store
                            .list()
                            .into_iter()
                            .rev()
                            .find(|artifact| !existing_artifact_ids.contains(&artifact.id))
                            .map(|r| r.uri)
                    });
                    let history_uri =
                        (!transcript.is_empty()).then(|| format!("history://{actor_id}"));
                    (artifact_uri, history_uri)
                }
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

        let history_content = if transcript.is_empty() {
            None
        } else {
            Some(transcript)
        };

        Ok(ActorDigest {
            actor_id,
            summary,
            artifact_uri,
            history_uri,
            history_content,
        })
    }
}

fn last_artifact_uri_in_text(text: &str) -> Option<String> {
    let marker = "artifact://";
    let mut cursor = 0;
    let mut found = None;
    while let Some(offset) = text[cursor..].find(marker) {
        let start = cursor + offset;
        let id_start = start + marker.len();
        let id_len = text[id_start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .map(char::len_utf8)
            .sum::<usize>();
        if id_len > 0 {
            found = Some(text[start..id_start + id_len].to_string());
            cursor = id_start + id_len;
        } else {
            cursor = id_start;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_artifact_uri_in_text_uses_child_transcript_order() {
        let text = "[cell_result] first artifact://0\n[text] later\n[cell_result] artifact://12,";
        assert_eq!(
            last_artifact_uri_in_text(text).as_deref(),
            Some("artifact://12")
        );
    }
}
