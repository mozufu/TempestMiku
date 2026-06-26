//! TempestMiku core.
//!
//! The protocol skeleton for a code-execution agent: message types, a streaming agent
//! loop built around a single `execute(code)` tool, and the `LlmClient` / `Sandbox`
//! abstractions the satellite crates implement. Streaming is the one transport here
//! (see [`LlmClient::chat_stream`]); the non-streaming [`LlmClient::chat`] just drains it.

mod agent;
mod error;
mod llm;
mod message;
mod prompt;
mod sandbox;
mod shape;
mod sink;
mod stream;

pub use agent::{Agent, AgentConfig, Protocol};
pub use error::{Error, Result};
pub use llm::{ChatRequest, LlmClient, Usage};
pub use message::{FunctionSpec, Message, Role, ToolCall, ToolChoice, ToolSpec};
pub use prompt::DEFAULT_SYSTEM_PROMPT;
pub use sandbox::{CellBudget, EvalOutput, Sandbox, Session, SessionConfig};
pub use shape::{shape_result, shape_result_capped};
pub use sink::{EventSink, NullSink};
pub use stream::{Accumulator, AssistantTurn, ExecuteCall, StreamEvent};
