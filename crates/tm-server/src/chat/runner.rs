use std::{
    sync::{Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use tm_core::{Agent, AgentConfig, EventSink, LlmClient, Sandbox, ToolMediator};
use tm_modes::ModeId;
use tm_sandbox::{DenoSandbox, DenoSandboxOptions};
use uuid::Uuid;

use crate::{Result, ServerError};

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
