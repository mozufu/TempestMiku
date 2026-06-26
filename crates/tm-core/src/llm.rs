use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::{Accumulator, AssistantTurn, Message, Result, StreamEvent, ToolChoice, ToolSpec};

/// A single chat-completions request, backend-agnostic.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// Token accounting for a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// An OpenAI-compatible chat backend.
///
/// Streaming is the single source of truth: [`chat_stream`](LlmClient::chat_stream) is the only
/// required method, and [`chat`](LlmClient::chat) is a provided convenience that drains the stream
/// into a finished turn. This keeps day-1 streaming and the non-streaming path from drifting apart.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Open a streamed completion. Each item is one [`StreamEvent`].
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>>;

    /// Drain a stream into a complete [`AssistantTurn`]. Default-implemented over
    /// [`chat_stream`](LlmClient::chat_stream); backends rarely need to override it.
    async fn chat(&self, req: &ChatRequest) -> Result<AssistantTurn> {
        let mut stream = self.chat_stream(req).await?;
        let mut acc = Accumulator::new();
        while let Some(ev) = stream.next().await {
            acc.push(ev?);
        }
        Ok(acc.into_turn())
    }
}
