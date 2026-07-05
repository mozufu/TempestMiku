use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use serde_json::json;
use tm_artifacts::ArtifactStore;
use tm_core::{Agent, AgentConfig, EventSink, InboxDrain, LlmClient, Sandbox};
use tm_host::CapabilityGrants;
use tm_persona::ModeId;
use tm_sandbox::{DenoSandbox, DenoSandboxOptions};
use uuid::Uuid;

use tm_agents::{ActorDigest, ActorError, ActorExecutor, ActorSpec};

use crate::{Result, ServerError, SessionEvent, Store, StoreEvent};

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
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String>;
}

#[async_trait]
impl<T: ChatRunner + ?Sized> ChatRunner for Arc<T> {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        (**self).run_turn(turn, sink).await
    }
}

struct AgentRequest {
    turn: ChatTurn,
    sink: Arc<dyn EventSink + Send + Sync>,
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
                    .block_on(agent.run(&request.turn.user_prompt, request.sink.as_ref()))
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
                    .block_on(agent.run(&request.turn.user_prompt, request.sink.as_ref()))
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
            sandbox_factory: Arc::new(move |session_id, _actor_id, _grants| {
                sandbox_factory(session_id)
            }),
            artifact_root,
            actor_roster: None,
        }
    }

    pub fn with_actor_context(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid, Option<&str>, &CapabilityGrants) -> Arc<dyn Sandbox>
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
    ) -> Result<String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| ServerError::Store("agent sender lock poisoned".to_string()))?;
            sender
                .send(AgentRequest { turn, sink, reply })
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
    ) -> Result<String> {
        match self {
            Self::Echo(r) => r.run_turn(turn, sink).await,
            Self::Agent(r) => r.run_turn(turn, sink).await,
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
    sandbox_factory:
        Arc<dyn Fn(Uuid, Option<&str>, &CapabilityGrants) -> Arc<dyn Sandbox> + Send + Sync>,
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
        let artifact_root = self.artifact_root.clone();
        let child_session_id = Uuid::new_v4();
        let sandbox =
            (self.sandbox_factory)(child_session_id, Some(actor_id.as_str()), &spec.grants);
        let inbox = self.actor_roster.as_ref().map(|roster| ActorMailboxDrain {
            roster: Arc::clone(roster),
            actor_id: actor_id.clone(),
        });

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
                .block_on(agent.run_with_inbox(
                    &task,
                    &sink,
                    inbox.as_ref().map(|inbox| inbox as &dyn InboxDrain),
                ))
                .map_err(|e| ActorError::Execution(e.to_string()));
            let transcript = sink.into_transcript();
            let _ = tx.send(result.map(|summary| (summary, transcript)));
        });

        let (summary, transcript) = rx
            .await
            .map_err(|_| ActorError::Execution("actor worker dropped response".to_string()))??;

        // Populate artifact_uri from child cell-spills; write transcript as history_uri.
        let (artifact_uri, history_uri) = if let Some(ref root) = artifact_root {
            match ArtifactStore::open(root, &child_session_id.to_string()) {
                Ok(store) => {
                    let artifact_uri = store.list().into_iter().next().map(|r| r.uri);
                    let history_uri = if !transcript.is_empty() {
                        store
                            .put_text(&transcript, Some("transcript".to_string()), "text/plain")
                            .ok()
                            .map(|r| r.uri)
                    } else {
                        None
                    };
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
