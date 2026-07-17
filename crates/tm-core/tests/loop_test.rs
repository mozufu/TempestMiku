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

struct CountingSession {
    evaluations: usize,
}

#[async_trait(?Send)]
impl Session for CountingSession {
    async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        self.evaluations += 1;
        Ok(EvalOutput {
            result: Some(Value::from(self.evaluations)),
            ..EvalOutput::default()
        })
    }

    async fn reset(&mut self) -> Result<()> {
        self.evaluations = 0;
        Ok(())
    }
}

#[derive(Default)]
struct BatchSession {
    batches: Vec<Vec<String>>,
}

#[async_trait(?Send)]
impl Session for BatchSession {
    async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        panic!("agent loop should use eval_batch for native execute calls")
    }

    async fn eval_batch(
        &mut self,
        codes: &[String],
        _budget: CellBudget,
    ) -> Result<Vec<EvalOutput>> {
        self.batches.push(codes.to_vec());
        Ok(codes
            .iter()
            .map(|code| EvalOutput {
                result: Some(Value::String(code.clone())),
                ..EvalOutput::default()
            })
            .collect())
    }

    async fn reset(&mut self) -> Result<()> {
        self.batches.clear();
        Ok(())
    }
}

#[test]
fn default_prompt_teaches_tm_repl_discovery_contract() {
    assert!(DEFAULT_SYSTEM_PROMPT.contains("@tools.search"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("help @capability"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("agents.parallel"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("do not spend turns rediscovering a capability"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("not JavaScript"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("tm-conformance-v2"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("at most 16 `execute` calls"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("ordered left-to-right"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("custom persona, mode, and actor prompts"));
    assert!(!TM_RUNTIME_BOOT_CONTRACT.contains("fs.patch"));
    assert!(!TM_RUNTIME_BOOT_CONTRACT.contains("stale tag"));

    let description = ToolSpec::execute().function.description;
    assert!(description.contains("tm-conformance-v2"));
    assert!(description.contains("@capability"));
    assert!(description.contains("@tools.search"));
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
    fn active() -> Self {
        Self(AtomicBool::new(false))
    }

    fn cancelled() -> Self {
        Self(AtomicBool::new(true))
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
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

struct FailingSink(&'static str);

impl FailingSink {
    fn at(&self, point: &str) -> Result<()> {
        if self.0 == point {
            Err(Error::EventSink(point.to_string()))
        } else {
            Ok(())
        }
    }
}

impl EventSink for FailingSink {
    fn try_on_reasoning(&self, _delta: &str) -> Result<()> {
        self.at("reasoning")
    }

    fn try_on_text(&self, _delta: &str) -> Result<()> {
        self.at("text")
    }

    fn try_on_tool_call(&self, _name: &str) -> Result<()> {
        self.at("tool_call")
    }

    fn try_on_cell_start(&self, _code: &str) -> Result<()> {
        self.at("cell_start")
    }

    fn try_on_cell_result(&self, _shaped: &str) -> Result<()> {
        self.at("cell_result")
    }

    fn try_on_final(&self, _text: &str) -> Result<()> {
        self.at("final")
    }

    fn try_on_turn_end(&self) -> Result<()> {
        self.at("turn_end")
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
async fn native_tool_loop_executes_parallel_batch_and_shapes_results_in_response_order() {
    let scripts = vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_1".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"display('first')"}"#.into()),
            },
            StreamEvent::ToolCall {
                index: 1,
                id: Some("call_2".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"display('second')"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("done".into()),
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
    let mut session = BatchSession::default();

    assert_eq!(
        agent
            .run_with_session("run both", &[], &mut session, &sink)
            .await
            .unwrap(),
        "done"
    );
    assert_eq!(
        session.batches,
        vec![vec![
            "display('first')".to_string(),
            "display('second')".to_string()
        ]]
    );

    let events = sink.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "cell_start")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "cell_result")
            .count(),
        2
    );

    let requests = llm.requests.lock();
    let tool_results: Vec<_> = requests[1]
        .iter()
        .filter(|message| message.role == Role::Tool)
        .collect();
    assert_eq!(tool_results.len(), 2);
    assert_eq!(tool_results[0].tool_call_id.as_deref(), Some("call_1"));
    assert!(tool_results[0].content.contains("display('first')"));
    assert_eq!(tool_results[1].tool_call_id.as_deref(), Some("call_2"));
    assert!(tool_results[1].content.contains("display('second')"));
}

#[tokio::test]
async fn open_session_path_includes_prior_history_and_reuses_session_state() {
    let execute_turn = |id: &str| {
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some(id.to_string()),
                name: Some("execute".to_string()),
                arguments: Some(r#"{"code":"display('count')"}"#.to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ]
    };
    let final_turn = |text: &str| {
        vec![
            StreamEvent::Text(text.to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ]
    };
    let llm = Arc::new(FakeLlm::new(vec![
        execute_turn("call_1"),
        final_turn("first"),
        execute_turn("call_2"),
        final_turn("second"),
    ]));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let prior = vec![
        Message::user("earlier question"),
        Message::assistant("earlier answer"),
    ];
    let mut session = CountingSession { evaluations: 0 };

    assert_eq!(
        agent
            .run_with_session("first current question", &prior, &mut session, &sink)
            .await
            .unwrap(),
        "first"
    );
    assert_eq!(
        agent
            .run_with_session("second current question", &[], &mut session, &sink)
            .await
            .unwrap(),
        "second"
    );
    assert_eq!(session.evaluations, 2);

    let requests = llm.requests.lock();
    assert_eq!(requests[0][0].role, Role::System);
    assert_eq!(&requests[0][1..3], prior.as_slice());
    assert_eq!(requests[0][3], Message::user("first current question"));
}

#[tokio::test]
async fn fallible_sink_errors_abort_every_agent_emission_point() {
    let final_script = || {
        vec![
            StreamEvent::Reasoning("private".to_string()),
            StreamEvent::Text("answer".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ]
    };
    let tool_script = || {
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_sink".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(r#"{"code":"display(1)"}"#.to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ]
    };

    for point in ["reasoning", "text", "turn_end", "final"] {
        let agent = Agent::new(
            Arc::new(FakeLlm::new(vec![final_script()])),
            Arc::new(EchoSandbox),
            config(Protocol::NativeTool),
        );
        let error = agent.run("emit", &FailingSink(point)).await.unwrap_err();
        assert!(matches!(error, Error::EventSink(ref failed) if failed == point));
    }

    for point in ["tool_call", "cell_start", "cell_result"] {
        let agent = Agent::new(
            Arc::new(FakeLlm::new(vec![tool_script(), final_script()])),
            Arc::new(EchoSandbox),
            config(Protocol::NativeTool),
        );
        let error = agent.run("emit", &FailingSink(point)).await.unwrap_err();
        assert!(matches!(error, Error::EventSink(ref failed) if failed == point));
    }
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
        .run_with_inbox("wait for worker", &sink, Some(&inbox))
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
        .run_with_controls("please do work", &sink, None, Some(&cancellation))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Cancelled));
    assert!(llm.requests.lock().is_empty());
    assert!(sink.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_stream() {
    struct PendingLlm;

    #[async_trait]
    impl LlmClient for PendingLlm {
        async fn chat_stream(
            &self,
            _req: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            Ok(Box::pin(stream::pending()))
        }
    }

    let agent = Agent::new(
        Arc::new(PendingLlm),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let run = agent.run_with_controls("please wait forever", &sink, None, Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_llm_connection() {
    struct PendingConnectLlm;

    #[async_trait]
    impl LlmClient for PendingConnectLlm {
        async fn chat_stream(
            &self,
            _req: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            futures::future::pending::<()>().await;
            unreachable!("pending LLM connection returned")
        }
    }

    let agent = Agent::new(
        Arc::new(PendingConnectLlm),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let run = agent.run_with_controls("connect", &sink, None, Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(sink.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_inbox_wait() {
    struct PendingInbox;

    #[async_trait]
    impl InboxDrain for PendingInbox {
        async fn drain(&self) -> Result<Vec<String>> {
            futures::future::pending().await
        }
    }

    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::Text("must not run".to_string()),
        StreamEvent::Finish {
            reason: Some("stop".to_string()),
        },
    ]]));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let inbox = PendingInbox;
    let run = agent.run_with_controls("wait", &sink, Some(&inbox), Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(llm.requests.lock().is_empty());
    assert!(sink.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_session_eval() {
    struct PendingEvalSandbox;
    struct PendingEvalSession;

    #[async_trait]
    impl Sandbox for PendingEvalSandbox {
        async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
            Ok(Box::new(PendingEvalSession))
        }
    }

    #[async_trait(?Send)]
    impl Session for PendingEvalSession {
        async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
            futures::future::pending().await
        }

        async fn reset(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some("call_pending_eval".to_string()),
            name: Some("execute".to_string()),
            arguments: Some(r#"{"code":"await pendingHostWait()"}"#.to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]]));
    let agent = Agent::new(
        llm,
        Arc::new(PendingEvalSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let run = agent.run_with_controls("evaluate", &sink, None, Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(sink.events().contains(&"cell_start".to_string()));
    assert!(!sink.events().contains(&"cell_result".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_aware_session_finishes_its_terminal_protocol_before_agent_returns() {
    struct CooperativeSandbox {
        cancellation: Arc<FlagCancellation>,
        terminalized: Arc<AtomicBool>,
    }
    struct CooperativeSession {
        cancellation: Arc<FlagCancellation>,
        terminalized: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Sandbox for CooperativeSandbox {
        async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
            Ok(Box::new(CooperativeSession {
                cancellation: Arc::clone(&self.cancellation),
                terminalized: Arc::clone(&self.terminalized),
            }))
        }
    }

    #[async_trait(?Send)]
    impl Session for CooperativeSession {
        fn handles_cancellation(&self) -> bool {
            true
        }

        async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
            self.cancellation.cancelled().await;
            // Model the runtime's bounded terminal-persistence/commit shield. An outer race would
            // return on cancellation and drop this future before the protocol completes.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            self.terminalized.store(true, Ordering::SeqCst);
            Err(Error::Cancelled)
        }

        async fn reset(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some("call_cooperative_eval".to_string()),
            name: Some("execute".to_string()),
            arguments: Some(r#"{"code":"await cooperativeHostWait()"}"#.to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]]));
    let cancellation = Arc::new(FlagCancellation::active());
    let terminalized = Arc::new(AtomicBool::new(false));
    let agent = Agent::new(
        llm,
        Arc::new(CooperativeSandbox {
            cancellation: Arc::clone(&cancellation),
            terminalized: Arc::clone(&terminalized),
        }),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let run = agent.run_with_controls("evaluate", &sink, None, Some(cancellation.as_ref()));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };

    let (result, ()) = tokio::join!(run, cancel);

    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(
        terminalized.load(Ordering::SeqCst),
        "agent returned before the session completed terminal persistence"
    );
}

#[tokio::test]
async fn rejects_oversized_or_malformed_tool_call_batches_before_execution() {
    let mut oversized = (0..17)
        .map(|index| StreamEvent::ToolCall {
            index,
            id: Some(format!("call_{index}")),
            name: Some("execute".into()),
            arguments: Some(format!(r#"{{"code":"{index}"}}"#)),
        })
        .collect::<Vec<_>>();
    oversized.push(StreamEvent::Finish {
        reason: Some("tool_calls".into()),
    });
    let cases = vec![
        oversized,
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_valid".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"1"}"#.into()),
            },
            StreamEvent::ToolCall {
                index: 1,
                id: Some("call_missing_name".into()),
                name: None,
                arguments: Some(r#"{"code":"2"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_duplicate".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"1"}"#.into()),
            },
            StreamEvent::ToolCall {
                index: 1,
                id: Some("call_duplicate".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"2"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("must not become a final answer".into()),
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_missing_name".into()),
                name: None,
                arguments: Some(r#"{"code":"1"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_other".into()),
                name: Some("mode_suggest".into()),
                arguments: Some(r#"{"target_mode":"serious_engineer"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_bad".into()),
                name: Some("execute".into()),
                arguments: Some("{}".into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_extra".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"1","extra":true}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_empty".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"  \n"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
    ];

    for script in cases {
        let llm = Arc::new(FakeLlm::new(vec![script]));
        let agent = Agent::new(llm, Arc::new(EchoSandbox), config(Protocol::NativeTool));
        let sink = CaptureSink::default();
        let result = agent.run("bad tool turn", &sink).await;
        assert!(matches!(result, Err(Error::Protocol(_))));
        assert!(!sink.events().contains(&"cell_start".to_string()));
    }
}

#[tokio::test]
async fn compatibility_eval_batch_stops_after_first_session_failure() {
    struct FailsSecondSession {
        calls: Vec<String>,
    }

    #[async_trait(?Send)]
    impl Session for FailsSecondSession {
        async fn eval(&mut self, code: &str, _budget: CellBudget) -> Result<EvalOutput> {
            self.calls.push(code.to_string());
            if self.calls.len() == 2 {
                return Err(Error::Sandbox("second cell failed".to_string()));
            }
            Ok(EvalOutput {
                result: Some(Value::String(code.to_string())),
                ..EvalOutput::default()
            })
        }

        async fn reset(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let script = vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some("call_1".into()),
            name: Some("execute".into()),
            arguments: Some(r#"{"code":"first"}"#.into()),
        },
        StreamEvent::ToolCall {
            index: 1,
            id: Some("call_2".into()),
            name: Some("execute".into()),
            arguments: Some(r#"{"code":"second"}"#.into()),
        },
        StreamEvent::ToolCall {
            index: 2,
            id: Some("call_3".into()),
            name: Some("execute".into()),
            arguments: Some(r#"{"code":"third"}"#.into()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".into()),
        },
    ];
    let agent = Agent::new(
        Arc::new(FakeLlm::new(vec![script])),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let mut session = FailsSecondSession { calls: Vec::new() };

    let result = agent
        .run_with_session("run batch", &[], &mut session, &sink)
        .await;

    assert!(matches!(result, Err(Error::Sandbox(ref message)) if message == "second cell failed"));
    assert_eq!(session.calls, vec!["first", "second"]);
    assert_eq!(
        sink.events()
            .iter()
            .filter(|event| event.as_str() == "cell_start")
            .count(),
        3
    );
}

#[tokio::test]
async fn native_requests_advertise_exactly_execute() {
    struct InspectingLlm(Arc<Mutex<Vec<ToolSpec>>>);

    #[async_trait]
    impl LlmClient for InspectingLlm {
        async fn chat_stream(
            &self,
            request: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            *self.0.lock() = request.tools.clone();
            Ok(Box::pin(stream::iter([
                Ok(StreamEvent::Text("done".to_string())),
                Ok(StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                }),
            ])))
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let agent = Agent::new(
        Arc::new(InspectingLlm(Arc::clone(&seen))),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    agent.run("hello", &CaptureSink::default()).await.unwrap();

    let tools = seen.lock();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].function.name, "execute");
}

#[tokio::test]
async fn tm_requests_append_language_contract_and_advertise_tm_execute() {
    struct InspectingLlm(Arc<Mutex<Option<(String, String)>>>);

    #[async_trait]
    impl LlmClient for InspectingLlm {
        async fn chat_stream(
            &self,
            request: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            *self.0.lock() = Some((
                request.messages[0].content.clone(),
                request.tools[0].function.description.clone(),
            ));
            Ok(Box::pin(stream::iter([
                Ok(StreamEvent::Text("done".to_string())),
                Ok(StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                }),
            ])))
        }
    }

    let seen = Arc::new(Mutex::new(None));
    let mut cfg = config(Protocol::NativeTool);
    cfg.system_prompt = "custom persona prompt".into();
    let agent = Agent::new(
        Arc::new(InspectingLlm(Arc::clone(&seen))),
        Arc::new(EchoSandbox),
        cfg,
    );
    agent.run("hello", &CaptureSink::default()).await.unwrap();

    let seen = seen.lock();
    let (prompt, description) = seen.as_ref().unwrap();
    assert!(prompt.starts_with("## Immutable tm runtime contract"));
    assert!(prompt.contains("custom persona prompt"));
    assert!(prompt.contains("tm-conformance-v2"));
    assert!(prompt.contains("separate top-level forms"));
    assert!(!prompt.contains("fs.patch"));
    assert!(!description.contains("JavaScript/TypeScript"));
    assert!(description.contains("tm-conformance-v2"));
}
