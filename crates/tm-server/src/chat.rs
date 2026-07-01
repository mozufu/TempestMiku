use std::{
    sync::{Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use serde_json::json;
use tm_core::{Agent, AgentConfig, EventSink, LlmClient, Sandbox};
use tm_sandbox::{DenoSandbox, DenoSandboxOptions};
use uuid::Uuid;

use crate::{Result, ServerError, SessionEvent, Store, StoreEvent};

#[async_trait]
pub trait ChatRunner: Send + Sync + 'static {
    async fn run_turn(
        &self,
        session_id: Uuid,
        user: String,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String>;
}

#[async_trait]
impl<T: ChatRunner + ?Sized> ChatRunner for Arc<T> {
    async fn run_turn(
        &self,
        session_id: Uuid,
        user: String,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        (**self).run_turn(session_id, user, sink).await
    }
}

struct AgentRequest {
    session_id: Uuid,
    user: String,
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
                    .block_on(agent.run(&request.user, request.sink.as_ref()))
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
                let sandbox = sandbox_factory(request.session_id);
                let agent = Agent::new(Arc::clone(&llm), sandbox, cfg.clone());
                let result = runtime
                    .block_on(agent.run(&request.user, request.sink.as_ref()))
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
        session_id: Uuid,
        user: String,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        {
            let sender = self
                .sender
                .lock()
                .map_err(|_| ServerError::Store("agent sender lock poisoned".to_string()))?;
            sender
                .send(AgentRequest {
                    session_id,
                    user,
                    sink,
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
        _session_id: Uuid,
        user: String,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        let text = format!("Miku heard: {user}");
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
        session_id: Uuid,
        user: String,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        match self {
            Self::Echo(r) => r.run_turn(session_id, user, sink).await,
            Self::Agent(r) => r.run_turn(session_id, user, sink).await,
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
