use std::{
    sync::{Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use serde_json::json;
use tm_core::{Agent, AgentConfig, EventSink, LlmClient, Sandbox};
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
    sandbox_factory: Arc<dyn Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync>,
}

impl ChatActorExecutor {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        Self {
            llm,
            cfg,
            sandbox_factory: Arc::new(sandbox_factory),
        }
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
        let sandbox = (self.sandbox_factory)(Uuid::new_v4());

        let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<String, ActorError>>();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("actor worker runtime builds");
            let agent = Agent::new(llm, sandbox, cfg);
            let result = runtime
                .block_on(agent.run(&task, &tm_core::NullSink))
                .map_err(|e| ActorError::Execution(e.to_string()));
            let _ = tx.send(result);
        });

        let summary = rx
            .await
            .map_err(|_| ActorError::Execution("actor worker dropped response".to_string()))??;

        Ok(ActorDigest {
            actor_id,
            summary,
            artifact_uri: None,
            history_uri: None, // transcript persistence deferred to P3.3
        })
    }
}
