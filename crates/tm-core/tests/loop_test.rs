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
    EvalOutput, EventSink, FunctionSpec, InboxDrain, LlmClient, Message, Protocol, Result, Role,
    Sandbox, Session, SessionConfig, StreamEvent, ToolMediator, ToolSpec,
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

#[test]
fn default_prompt_teaches_js_ts_repl_discovery_contract() {
    assert!(DEFAULT_SYSTEM_PROMPT.contains("JavaScript/TypeScript"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("Top-level `await`"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("await tools.search"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("await tools.docs"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("agents.parallel"));

    let description = ToolSpec::execute().function.description;
    assert!(description.contains("JavaScript/TypeScript"));
    assert!(description.contains("top-level await"));
    assert!(description.contains("await tools.search"));
}

struct StaticInbox {
    messages: Mutex<VecDeque<String>>,
}

impl StaticInbox {
    fn new(messages: Vec<&str>) -> Self {
        Self {
            messages: Mutex::new(messages.into_iter().map(str::to_string).collect()),
        }
    }
}

#[async_trait]
impl InboxDrain for StaticInbox {
    async fn drain(&self) -> Result<Vec<String>> {
        Ok(self.messages.lock().drain(..).collect())
    }
}

struct FlagCancellation(AtomicBool);

impl FlagCancellation {
    fn cancelled() -> Self {
        Self(AtomicBool::new(true))
    }
}

impl CancellationToken for FlagCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
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

#[tokio::test]
async fn run_with_inbox_drains_before_model_turn() {
    let scripts = vec![vec![
        StreamEvent::Text("I saw the inbox.".into()),
        StreamEvent::Finish {
            reason: Some("stop".into()),
        },
    ]];

    let llm = Arc::new(FakeLlm::new(scripts));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let inbox = StaticInbox::new(vec!["from: Worker\ntext: done"]);

    let answer = agent
        .run_with_inbox("wait for worker", &sink, Some(&inbox), None)
        .await
        .unwrap();
    assert_eq!(answer, "I saw the inbox.");

    let requests = llm.requests.lock();
    let inbox_msg = requests[0]
        .iter()
        .find(|m| m.role == Role::User && m.content.contains("[actor inbox]"))
        .expect("inbox message should be appended before the first turn");
    assert!(inbox_msg.content.contains("from: Worker"));
    assert!(inbox_msg.content.contains("text: done"));
}

#[tokio::test]
async fn run_with_controls_stops_before_model_turn_when_cancelled() {
    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::Text("should not stream".into()),
        StreamEvent::Finish {
            reason: Some("stop".into()),
        },
    ]]));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::cancelled();

    let err = agent
        .run_with_controls("please do work", &sink, None, None, Some(&cancellation))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Cancelled));
    assert!(llm.requests.lock().is_empty());
    assert!(sink.events().is_empty());
}

/// A [`ToolMediator`] that advertises one tool, records every call it handles, and returns a
/// canned result — enough to prove the loop's mediated-tool-call seam without a real product
/// implementation (e.g. `ModeSuggestMediator` in tm-server).
#[derive(Default)]
struct MockMediator {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl ToolMediator for MockMediator {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            kind: "function".into(),
            function: FunctionSpec {
                name: "mode_suggest".into(),
                description: "propose a mode switch".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
        }]
    }

    async fn handle(&self, name: &str, arguments: &Value) -> Result<String> {
        self.calls
            .lock()
            .push((name.to_string(), arguments.clone()));
        Ok("switched".to_string())
    }
}

#[tokio::test]
async fn mediated_tool_call_feeds_back_result_and_continues_to_final() {
    // Turn 1: the model calls the mediator's tool instead of `execute`. Turn 2: final answer.
    let scripts = vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_ms".into()),
                name: Some("mode_suggest".into()),
                arguments: Some(
                    r#"{"target_mode":"serious_engineer","reason":"fix a bug"}"#.into(),
                ),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("Done.".into()),
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
    let mediator = MockMediator::default();

    let answer = agent
        .run_with_inbox("please fix the bug", &sink, None, Some(&mediator))
        .await
        .unwrap();
    assert_eq!(answer, "Done.");

    // The seam fired exactly once, with the model's exact arguments.
    let calls = mediator.calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "mode_suggest");
    assert_eq!(calls[0].1["target_mode"], "serious_engineer");
    assert_eq!(calls[0].1["reason"], "fix a bug");

    // A `Role::Tool` message keyed to the mediated call's id was appended before turn 2.
    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 2, "two turns => two requests");
    let tool_msg = requests[1]
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("tool result appended before turn 2");
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_ms"));
    assert_eq!(tool_msg.content, "switched");

    // No sandbox cell ever ran — this call never reached `execute`.
    assert!(!sink.events().contains(&"cell_start".to_string()));
}

#[tokio::test]
async fn mediator_error_becomes_a_tool_result_not_an_aborted_turn() {
    struct FailingMediator;

    #[async_trait]
    impl ToolMediator for FailingMediator {
        fn tool_specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                kind: "function".into(),
                function: FunctionSpec {
                    name: "mode_suggest".into(),
                    description: "propose a mode switch".into(),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                },
            }]
        }

        async fn handle(&self, _name: &str, _arguments: &Value) -> Result<String> {
            Err(Error::Sandbox("broker unavailable".into()))
        }
    }

    let scripts = vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_ms".into()),
                name: Some("mode_suggest".into()),
                arguments: Some(r#"{"target_mode":"serious_engineer"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("Continuing anyway.".into()),
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
    let mediator = FailingMediator;

    let answer = agent
        .run_with_inbox("please fix the bug", &sink, None, Some(&mediator))
        .await
        .unwrap();
    assert_eq!(
        answer, "Continuing anyway.",
        "one failed mediated call must not abort the turn"
    );

    let requests = llm.requests.lock();
    let tool_msg = requests[1]
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("a tool result was still appended after the mediator error");
    assert!(tool_msg.content.contains("broker unavailable"));
}
