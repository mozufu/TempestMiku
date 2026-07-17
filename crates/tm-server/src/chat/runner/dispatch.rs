use std::sync::Arc;

use async_trait::async_trait;
use tm_core::EventSink;
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{AgentChatRunner, ChatRunner, ChatTurn};

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
        sink.try_on_text(&text)
            .and_then(|()| sink.try_on_final(&text))
            .map_err(|err| ServerError::Store(err.to_string()))?;
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

    async fn promote_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        match self {
            Self::Echo(r) => r.promote_session(session_id, turn_id).await,
            Self::Agent(r) => r.promote_session(session_id, turn_id).await,
        }
    }

    async fn abort_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        match self {
            Self::Echo(r) => r.abort_session(session_id, turn_id).await,
            Self::Agent(r) => r.abort_session(session_id, turn_id).await,
        }
    }

    fn cancel_turn(&self, session_id: Uuid, turn_id: Uuid) {
        match self {
            Self::Echo(r) => r.cancel_turn(session_id, turn_id),
            Self::Agent(r) => r.cancel_turn(session_id, turn_id),
        }
    }
}
