use std::{
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use tm_core::{
    CellBudget, ChatRequest, Error, EvalOutput, Message, NullSink, Role, SessionConfig,
    StreamEvent, ToolChoice,
};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, FsMode, GrantDoc, HostFn, InvocationCtx, LinkedFolderConfig,
    LinkedFolders, ToolDocs,
};

use super::*;
use crate::{InMemoryStore, NewSession, PersistingEventSink, Store};

struct AlwaysApprove;

#[async_trait]
impl ApprovalPolicy for AlwaysApprove {
    async fn request(
        &self,
        _action: &str,
        _timeout: Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approved)
    }
}

struct ApprovedPatch {
    calls: Arc<AtomicUsize>,
    docs: ToolDocs,
}

impl ApprovedPatch {
    fn new(calls: Arc<AtomicUsize>) -> Self {
        Self {
            calls,
            docs: ToolDocs {
                name: "fs.patch".into(),
                namespace: "fs".into(),
                summary: "Apply a test patch".into(),
                description: None,
                signature: "fs.patch(args)".into(),
                args_schema: serde_json::json!({"type":"object"}),
                result_schema: Some(serde_json::json!({"type":"object"})),
                examples: Vec::new(),
                errors: Vec::new(),
                grants: vec![GrantDoc {
                    kind: "capability".into(),
                    description: "fs.patch".into(),
                }],
                sensitive: true,
                approval: "on-write".into(),
                since: "0.1".into(),
                stability: "stable".into(),
            },
        }
    }
}

#[async_trait]
impl HostFn for ApprovedPatch {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &InvocationCtx,
    ) -> tm_host::Result<serde_json::Value> {
        ctx.require_approval("fs.patch").await?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"applied": true, "args": args}))
    }
}

struct ReactiveLlm {
    requests: Mutex<Vec<Vec<Message>>>,
    code: String,
    dialectic_text: Mutex<String>,
}

impl Default for ReactiveLlm {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            code: "increment".to_string(),
            dialectic_text: Mutex::new(
                "Brian's approved profile preference is relevant as a tentative constraint."
                    .to_string(),
            ),
        }
    }
}

impl ReactiveLlm {
    fn with_code(code: impl Into<String>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            code: code.into(),
            dialectic_text: Mutex::new(
                "Brian's approved profile preference is relevant as a tentative constraint."
                    .to_string(),
            ),
        }
    }
}

#[async_trait]
impl LlmClient for ReactiveLlm {
    async fn chat_stream(
        &self,
        request: &ChatRequest,
    ) -> tm_core::Result<BoxStream<'static, tm_core::Result<StreamEvent>>> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let events = if request.tools.is_empty()
            && request.tool_choice == ToolChoice::None
            && request
                .messages
                .first()
                .is_some_and(|message| message.content.contains("bounded user-model synthesizer"))
        {
            vec![
                StreamEvent::Text(self.dialectic_text.lock().unwrap().clone()),
                StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                },
            ]
        } else if request
            .messages
            .last()
            .is_some_and(|message| message.role == Role::Tool)
        {
            vec![
                StreamEvent::Text("done".to_string()),
                StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                },
            ]
        } else {
            vec![
                StreamEvent::ToolCall {
                    index: 0,
                    id: Some("call_state".to_string()),
                    name: Some("execute".to_string()),
                    arguments: Some(serde_json::json!({ "code": self.code }).to_string()),
                },
                StreamEvent::Finish {
                    reason: Some("tool_calls".to_string()),
                },
            ]
        };
        Ok(Box::pin(stream::iter(
            events.into_iter().map(Ok::<StreamEvent, Error>),
        )))
    }
}

struct StatefulSandbox {
    session_id: Uuid,
    opens: Arc<Mutex<Vec<(Uuid, String)>>>,
    evaluations: Arc<Mutex<Vec<(Uuid, usize)>>>,
    wall_budgets: Arc<Mutex<Vec<u64>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    delay: Duration,
}

#[async_trait]
impl Sandbox for StatefulSandbox {
    async fn open(&self, _cfg: SessionConfig) -> tm_core::Result<Box<dyn Session>> {
        self.opens.lock().unwrap().push((
            self.session_id,
            thread::current().name().unwrap_or("unnamed").to_string(),
        ));
        Ok(Box::new(StatefulSession {
            session_id: self.session_id,
            value: 0,
            evaluations: Arc::clone(&self.evaluations),
            wall_budgets: Arc::clone(&self.wall_budgets),
            active: Arc::clone(&self.active),
            max_active: Arc::clone(&self.max_active),
            drops: Arc::clone(&self.drops),
            delay: self.delay,
            _not_send: Rc::new(()),
        }))
    }
}

struct StatefulSession {
    session_id: Uuid,
    value: usize,
    evaluations: Arc<Mutex<Vec<(Uuid, usize)>>>,
    wall_budgets: Arc<Mutex<Vec<u64>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    delay: Duration,
    _not_send: Rc<()>,
}

impl Drop for StatefulSession {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait(?Send)]
impl Session for StatefulSession {
    async fn eval(&mut self, _code: &str, budget: CellBudget) -> tm_core::Result<EvalOutput> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.value += 1;
        self.wall_budgets.lock().unwrap().push(budget.wall_ms);
        self.evaluations
            .lock()
            .unwrap()
            .push((self.session_id, self.value));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(EvalOutput {
            result: Some(serde_json::Value::from(self.value)),
            ..EvalOutput::default()
        })
    }

    async fn reset(&mut self) -> tm_core::Result<()> {
        self.value = 0;
        Ok(())
    }
}

struct Harness {
    llm: Arc<ReactiveLlm>,
    opens: Arc<Mutex<Vec<(Uuid, String)>>>,
    evaluations: Arc<Mutex<Vec<(Uuid, usize)>>>,
    wall_budgets: Arc<Mutex<Vec<u64>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

impl Harness {
    fn new() -> Self {
        Self {
            llm: Arc::new(ReactiveLlm::default()),
            opens: Arc::new(Mutex::new(Vec::new())),
            evaluations: Arc::new(Mutex::new(Vec::new())),
            wall_budgets: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn runner(&self, options: AgentChatRunnerOptions, delay: Duration) -> AgentChatRunner {
        let llm: Arc<dyn LlmClient> = self.llm.clone();
        let opens = Arc::clone(&self.opens);
        let evaluations = Arc::clone(&self.evaluations);
        let wall_budgets = Arc::clone(&self.wall_budgets);
        let active = Arc::clone(&self.active);
        let max_active = Arc::clone(&self.max_active);
        let drops = Arc::clone(&self.drops);
        AgentChatRunner::new_with_sandbox_factory_and_options(
            llm,
            AgentConfig {
                model: "fake".to_string(),
                max_turns: 3,
                ..AgentConfig::default()
            },
            options,
            move |turn| {
                Arc::new(StatefulSandbox {
                    session_id: turn.session_id,
                    opens: Arc::clone(&opens),
                    evaluations: Arc::clone(&evaluations),
                    wall_budgets: Arc::clone(&wall_budgets),
                    active: Arc::clone(&active),
                    max_active: Arc::clone(&max_active),
                    drops: Arc::clone(&drops),
                    delay,
                })
            },
        )
    }
}

#[derive(Default)]
struct ResetSink(AtomicUsize);

impl EventSink for ResetSink {
    fn on_runtime_reset(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct FailingResetSink;

impl EventSink for FailingResetSink {
    fn try_on_runtime_reset(&self) -> tm_core::Result<()> {
        Err(Error::EventSink("runtime_reset backpressure".to_string()))
    }
}

struct FailingCellResultSink;

impl EventSink for FailingCellResultSink {
    fn try_on_cell_result(&self, _shaped: &str) -> tm_core::Result<()> {
        Err(Error::EventSink(
            "cell result persistence failed".to_string(),
        ))
    }
}

struct FailingBarrierSink;

impl EventSink for FailingBarrierSink {
    fn try_on_event_barrier_confirmed(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tm_core::Result<()>> + Send + '_>> {
        Box::pin(async {
            Err(Error::EventSink(
                "event persistence barrier failed".to_string(),
            ))
        })
    }
}

struct AsyncConfirmedFailureSink;

impl EventSink for AsyncConfirmedFailureSink {
    fn try_on_runtime_event(
        &self,
        _event_type: &str,
        _payload: &serde_json::Value,
    ) -> tm_core::Result<()> {
        Ok(())
    }

    fn try_on_runtime_event_confirmed<'a>(
        &'a self,
        _event_type: &'a str,
        _payload: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tm_core::Result<()>> + Send + 'a>> {
        Box::pin(async {
            tokio::task::yield_now().await;
            Err(Error::EventSink(
                "async runtime persistence failed".to_string(),
            ))
        })
    }
}

#[derive(Default)]
struct RuntimeSink(Mutex<Vec<(String, serde_json::Value)>>);

impl EventSink for RuntimeSink {
    fn on_runtime_event(&self, event_type: &str, payload: &serde_json::Value) {
        self.0
            .lock()
            .unwrap()
            .push((event_type.to_string(), payload.clone()));
    }
}

fn chat_turn(session_id: Uuid) -> ChatTurn {
    ChatTurn {
        session_id,
        durable_turn_id: None,
        user_prompt: "advance state".to_string(),
        mode: ModeId::from("general"),
        owner_subject: "brian".to_string(),
        project_id: None,
        memory_scope: "global".to_string(),
        system_prompt: "test system".to_string(),
        capabilities: Vec::new(),
        prior_messages: Vec::new(),
        dialectic: None,
        limits: ChatRunLimits::default(),
        deny_approvals: false,
        host_functions: Vec::new(),
        resource_handlers: Vec::new(),
    }
}

async fn run(runner: &AgentChatRunner, turn: ChatTurn) {
    let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(NullSink);
    assert_eq!(runner.run_turn(turn, sink).await.unwrap(), "done");
}

fn options(shard_count: usize, session_ttl: Duration) -> AgentChatRunnerOptions {
    AgentChatRunnerOptions {
        shard_count,
        session_ttl,
    }
}

mod runtime_events;
mod runtime_reset;
mod session_cache;
mod turn_config;
