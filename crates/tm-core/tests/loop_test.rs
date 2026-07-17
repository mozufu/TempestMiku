//! End-to-end agent-loop tests with no network: a scripted streaming `LlmClient`, an inline
//! echo sandbox, and a capturing sink. Exercises delta accumulation, the streaming sink, and
//! the `tool_call -> tool_result -> final` sequence for both protocols.

use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use parking_lot::Mutex;
use serde_json::Value;

use tm_core::{
    Agent, AgentConfig, CancellationToken, CellBudget, ChatRequest, DEFAULT_SYSTEM_PROMPT, Error,
    EvalOutput, EventSink, InboxDrain, LlmClient, Message, Protocol, Result, Role, Sandbox,
    Session, SessionConfig, StreamEvent, TM_RUNTIME_BOOT_CONTRACT, ToolSpec,
};

#[path = "loop_test/batch.rs"]
mod batch;
#[path = "loop_test/cancellation_session.rs"]
mod cancellation_session;
#[path = "loop_test/protocol.rs"]
mod protocol;
#[path = "loop_test/streaming.rs"]
mod streaming;
#[path = "loop_test/support.rs"]
mod support;
