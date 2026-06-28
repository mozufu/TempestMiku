//! End-to-end agent-loop tests with no network: a scripted streaming `LlmClient`, an inline
//! echo sandbox, and a capturing sink. Exercises delta accumulation, the streaming sink, and
//! the `tool_call -> tool_result -> final` sequence for both protocols.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use parking_lot::Mutex;
use serde_json::Value;

use tm_core::{
    Agent, AgentConfig, CellBudget, ChatRequest, Error, EvalOutput, EventSink, LlmClient, Message,
    Protocol, Result, Role, Sandbox, Session, SessionConfig, StreamEvent,
};

/// An `LlmClient` that replays a fixed script of stream events per turn and records every
/// request's message list for assertions.
struct FakeLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl FakeLlm {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmClient for FakeLlm {
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        self.requests.lock().push(req.messages.clone());
        let script = self.scripts.lock().pop_front().unwrap_or_default();
        Ok(Box::pin(stream::iter(
            script.into_iter().map(Ok::<StreamEvent, Error>),
        )))
    }
}

/// A sandbox that echoes the submitted code as its result — enough to prove the loop feeds the
/// cell output back to the model.
struct EchoSandbox;

#[async_trait]
impl Sandbox for EchoSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(EchoSession))
    }
}

struct EchoSession;

#[async_trait(?Send)]
impl Session for EchoSession {
    async fn eval(&mut self, code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        Ok(EvalOutput {
            stdout: String::new(),
            result: Some(Value::String(code.to_string())),
            error: None,
        })
    }

    async fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Records the streaming events the loop emits, in order.
#[derive(Default)]
struct CaptureSink {
    events: Mutex<Vec<String>>,
}

impl CaptureSink {
    fn events(&self) -> Vec<String> {
        self.events.lock().clone()
    }
}

impl EventSink for CaptureSink {
    fn on_text(&self, delta: &str) {
        self.events.lock().push(format!("text:{delta}"));
    }
    fn on_tool_call(&self, name: &str) {
        self.events.lock().push(format!("tool:{name}"));
    }
    fn on_cell_start(&self, _code: &str) {
        self.events.lock().push("cell_start".into());
    }
    fn on_cell_result(&self, _shaped: &str) {
        self.events.lock().push("cell_result".into());
    }
    fn on_final(&self, text: &str) {
        self.events.lock().push(format!("final:{text}"));
    }
}

fn config(protocol: Protocol) -> AgentConfig {
    AgentConfig {
        model: "fake".into(),
        protocol,
        ..AgentConfig::default()
    }
}

#[tokio::test]
async fn native_tool_loop_streams_and_executes() {
    // Turn 1: an `execute` tool call whose JSON arguments are split across two deltas.
    // Turn 2: the final answer, streamed token-by-token.
    let scripts = vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_1".into()),
                name: Some("execute".into()),
                arguments: Some("{\"code\":\"dis".into()),
            },
            StreamEvent::ToolCall {
                index: 0,
                id: None,
                name: None,
                arguments: Some("play(2+2)\"}".into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("The answer ".into()),
            StreamEvent::Text("is 4.".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
    ];

    let llm = Arc::new(FakeLlm::new(scripts));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();

    let answer = agent.run("compute 2+2", &sink).await.unwrap();
    assert_eq!(answer, "The answer is 4.");

    let events = sink.events();
    // The fragmented tool call resolved to a single named call.
    assert_eq!(
        events.iter().filter(|e| *e == "tool:execute").count(),
        1,
        "events: {events:?}"
    );
    assert!(events.contains(&"cell_start".to_string()));
    assert!(events.contains(&"cell_result".to_string()));
    // Assistant text streamed in order, as discrete deltas.
    let text: Vec<_> = events.iter().filter(|e| e.starts_with("text:")).collect();
    assert_eq!(text, vec!["text:The answer ", "text:is 4."]);
    assert_eq!(events.last().unwrap(), "final:The answer is 4.");

    // The loop appended a tool result keyed to the streamed call id, carrying the echoed code.
    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 2, "two turns => two requests");
    let tool_msg = requests[1]
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("tool result appended before turn 2");
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
    assert!(
        tool_msg.content.contains("display(2+2)"),
        "tool result should echo the executed code, got: {}",
        tool_msg.content
    );
}

#[tokio::test]
async fn fenced_block_loop_executes() {
    // Turn 1: a ```run fenced block, streamed in fragments. Turn 2: the final answer.
    let scripts = vec![
        vec![
            StreamEvent::Text("Let me compute.\n```run\n".into()),
            StreamEvent::Text("display(2 + 2)\n```\n".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
        vec![
            StreamEvent::Text("It is 4.".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
    ];

    let llm = Arc::new(FakeLlm::new(scripts));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::FencedBlock),
    );
    let sink = CaptureSink::default();

    let answer = agent.run("compute 2+2", &sink).await.unwrap();
    assert_eq!(answer, "It is 4.");

    // The parsed fence ran and its result was fed back as a user message (no tool channel).
    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 2);
    let fed_back = requests[1]
        .iter()
        .find(|m| m.role == Role::User && m.content.contains("[execution result]"))
        .expect("execution result fed back as a user message");
    assert!(fed_back.content.contains("display(2 + 2)"));
}
