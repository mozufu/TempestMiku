use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tm_core::{Agent, AgentConfig, EventSink, LlmClient, Message, Sandbox, Session};
use tm_host::{DefaultDenyApprovalPolicy, HostEventSink, HostFn};
use tm_lang::{TmSandbox, TmSandboxOptions, core_tm_grants};
use tm_modes::ModeId;
use uuid::Uuid;

use crate::{
    Result, ServerError,
    session_shards::{ThreadAffineShardPool, evict_expired_sessions, session_sweep_interval},
};

#[cfg(test)]
use crate::session_shards::shard_index;

#[derive(Clone)]
pub struct ChatTurn {
    pub session_id: Uuid,
    pub user_prompt: String,
    pub mode: ModeId,
    pub scope: String,
    pub system_prompt: String,
    pub capabilities: Vec<String>,
    pub prior_messages: Vec<Message>,
    pub limits: ChatRunLimits,
    pub deny_approvals: bool,
    /// Server-owned SDK handlers installed for this turn. Registration does not grant
    /// authority; `capabilities` remains the exact grant set enforced by the sandbox.
    pub host_functions: Vec<Arc<dyn HostFn>>,
}

impl std::fmt::Debug for ChatTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatTurn")
            .field("session_id", &self.session_id)
            .field("user_prompt", &self.user_prompt)
            .field("mode", &self.mode)
            .field("scope", &self.scope)
            .field("system_prompt", &self.system_prompt)
            .field("capabilities", &self.capabilities)
            .field("prior_messages", &self.prior_messages)
            .field("limits", &self.limits)
            .field("deny_approvals", &self.deny_approvals)
            .field(
                "host_functions",
                &self
                    .host_functions
                    .iter()
                    .map(|function| function.name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatRunLimits {
    pub max_turns: Option<usize>,
    pub cell_wall_ms: Option<u64>,
}

#[async_trait]
pub trait ChatRunner: Send + Sync + 'static {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String>;
}

#[async_trait]
impl<T: ChatRunner + ?Sized> ChatRunner for Arc<T> {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        (**self).run_turn(turn, sink).await
    }
}

struct AgentRequest {
    turn: ChatTurn,
    sink: Arc<dyn EventSink + Send + Sync>,
    reply: tokio::sync::oneshot::Sender<Result<String>>,
}

pub struct AgentChatRunner {
    shards: ThreadAffineShardPool<AgentRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentChatRunnerOptions {
    pub shard_count: usize,
    pub session_ttl: Duration,
}

impl Default for AgentChatRunnerOptions {
    fn default() -> Self {
        Self {
            shard_count: 4,
            session_ttl: Duration::from_secs(30 * 60),
        }
    }
}

type SandboxFactory =
    dyn Fn(&ChatTurn, Arc<dyn HostEventSink>) -> Arc<dyn Sandbox> + Send + Sync + 'static;

#[derive(Clone, PartialEq, Eq)]
struct SandboxProfile {
    mode: ModeId,
    scope: String,
    capabilities: Vec<String>,
    deny_approvals: bool,
    host_functions: Vec<String>,
}

impl From<&ChatTurn> for SandboxProfile {
    fn from(turn: &ChatTurn) -> Self {
        Self {
            mode: turn.mode.clone(),
            scope: turn.scope.clone(),
            capabilities: turn.capabilities.clone(),
            deny_approvals: turn.deny_approvals,
            host_functions: turn
                .host_functions
                .iter()
                .map(|function| function.name().to_string())
                .collect(),
        }
    }
}

struct CachedSession {
    sandbox: Arc<dyn Sandbox>,
    session: Box<dyn Session>,
    profile: SandboxProfile,
    last_used: Instant,
    event_proxy: Arc<SwappableHostEventSink>,
}

#[derive(Default)]
struct SwappableHostEventSink {
    sink: Mutex<Option<Arc<dyn EventSink + Send + Sync>>>,
}

impl SwappableHostEventSink {
    fn bind(&self, sink: Arc<dyn EventSink + Send + Sync>) {
        *self.sink.lock().expect("host event proxy lock poisoned") = Some(sink);
    }

    fn clear(&self) {
        self.sink
            .lock()
            .expect("host event proxy lock poisoned")
            .take();
    }
}

#[async_trait]
impl HostEventSink for SwappableHostEventSink {
    async fn emit(&self, event_type: &str, payload_json: serde_json::Value) -> tm_host::Result<()> {
        let sink = self
            .sink
            .lock()
            .expect("host event proxy lock poisoned")
            .clone();
        sink.as_ref().map_or(Ok(()), |sink| {
            sink.try_on_runtime_event(event_type, &payload_json)
                .map_err(|error| tm_host::HostError::HostCall(error.to_string()))
        })
    }
}

impl AgentChatRunner {
    pub fn new(agent: Agent) -> Self {
        let shards: ThreadAffineShardPool<AgentRequest> =
            ThreadAffineShardPool::<AgentRequest>::spawn_one("tm-agent-worker", move |receiver| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("agent worker runtime builds");
                for request in receiver {
                    let result = runtime
                        .block_on(agent.run_with_prior_messages_and_controls(
                            &request.turn.user_prompt,
                            &request.turn.prior_messages,
                            request.sink.as_ref(),
                            None,
                            None,
                        ))
                        .map_err(|err| ServerError::Store(err.to_string()));
                    let _ = request.reply.send(result);
                }
            });
        Self { shards }
    }

    pub fn tm(llm: Arc<dyn LlmClient>, cfg: AgentConfig, base_options: TmSandboxOptions) -> Self {
        Self::tm_with_options(llm, cfg, base_options, AgentChatRunnerOptions::default())
    }

    pub fn tm_with_options(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        base_options: TmSandboxOptions,
        runner_options: AgentChatRunnerOptions,
    ) -> Self {
        Self::new_with_host_event_sandbox_factory_and_options(
            llm,
            cfg,
            runner_options,
            move |turn, host_events| {
                let mut options = base_options.clone();
                options.session_id = turn.session_id.to_string();
                options.session_scope = Some(turn.scope.clone());
                options.grants = core_tm_grants().allow_many(turn.capabilities.iter().cloned());
                for function in &turn.host_functions {
                    options.host_registry.register(Arc::clone(function));
                }
                if turn.deny_approvals {
                    options.approval_policy = Arc::new(DefaultDenyApprovalPolicy);
                }
                options.host_event_sink = host_events;
                Arc::new(TmSandbox::new(options))
            },
        )
    }

    pub fn new_with_sandbox_factory(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(&ChatTurn) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_sandbox_factory_and_options(
            llm,
            cfg,
            AgentChatRunnerOptions::default(),
            sandbox_factory,
        )
    }

    pub fn new_with_sandbox_factory_and_options(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        runner_options: AgentChatRunnerOptions,
        sandbox_factory: impl Fn(&ChatTurn) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_host_event_sandbox_factory_and_options(
            llm,
            cfg,
            runner_options,
            move |turn, _host_events| sandbox_factory(turn),
        )
    }

    fn new_with_host_event_sandbox_factory_and_options(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        runner_options: AgentChatRunnerOptions,
        sandbox_factory: impl Fn(&ChatTurn, Arc<dyn HostEventSink>) -> Arc<dyn Sandbox>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let sandbox_factory: Arc<SandboxFactory> = Arc::new(sandbox_factory);
        let shards =
            ThreadAffineShardPool::spawn(runner_options.shard_count, "tm-agent-shard", move |_| {
                let llm = Arc::clone(&llm);
                let cfg = cfg.clone();
                let sandbox_factory = Arc::clone(&sandbox_factory);
                move |receiver| {
                    run_agent_shard(
                        receiver,
                        llm,
                        cfg,
                        sandbox_factory,
                        runner_options.session_ttl,
                    );
                }
            });
        Self { shards }
    }
}

fn run_agent_shard(
    receiver: mpsc::Receiver<AgentRequest>,
    llm: Arc<dyn LlmClient>,
    cfg: AgentConfig,
    sandbox_factory: Arc<SandboxFactory>,
    session_ttl: Duration,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("agent shard runtime builds");
    let mut sessions = HashMap::<Uuid, CachedSession>::new();
    let sweep_interval = session_sweep_interval(session_ttl);

    loop {
        let request = match receiver.recv_timeout(sweep_interval) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                evict_expired_sessions(&mut sessions, session_ttl, Instant::now(), |cached| {
                    cached.last_used
                });
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        evict_expired_sessions(&mut sessions, session_ttl, Instant::now(), |cached| {
            cached.last_used
        });
        let result = runtime.block_on(run_cached_turn(
            &request,
            &llm,
            &cfg,
            sandbox_factory.as_ref(),
            &mut sessions,
        ));
        let _ = request.reply.send(result);
    }
}

async fn run_cached_turn(
    request: &AgentRequest,
    llm: &Arc<dyn LlmClient>,
    base_cfg: &AgentConfig,
    sandbox_factory: &SandboxFactory,
    sessions: &mut HashMap<Uuid, CachedSession>,
) -> Result<String> {
    let session_id = request.turn.session_id;
    let profile = SandboxProfile::from(&request.turn);
    let replace_session = sessions
        .get(&session_id)
        .is_none_or(|cached| cached.profile != profile);
    if replace_session {
        let event_proxy = Arc::new(SwappableHostEventSink::default());
        event_proxy.bind(Arc::clone(&request.sink));
        let host_events: Arc<dyn HostEventSink> = event_proxy.clone();
        let sandbox = sandbox_factory(&request.turn, host_events);
        let session = sandbox
            .open(tm_core::SessionConfig::default())
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sessions.insert(
            session_id,
            CachedSession {
                sandbox,
                session,
                profile,
                last_used: Instant::now(),
                event_proxy,
            },
        );
    }

    let cached = sessions
        .get_mut(&session_id)
        .expect("cached session exists after open");
    cached.event_proxy.bind(Arc::clone(&request.sink));
    let mut cfg = base_cfg.clone();
    cfg.system_prompt = request.turn.system_prompt.clone();
    apply_turn_limits(&mut cfg, request.turn.limits);
    let agent = Agent::new(Arc::clone(llm), Arc::clone(&cached.sandbox), cfg);
    let result = async {
        if replace_session && !request.turn.prior_messages.is_empty() {
            request.sink.try_on_runtime_reset()?;
        }
        agent
            .run_with_session_and_controls(
                &request.turn.user_prompt,
                &request.turn.prior_messages,
                cached.session.as_mut(),
                request.sink.as_ref(),
                None,
                None,
            )
            .await
    }
    .await
    .map_err(|err| ServerError::Store(err.to_string()));
    cached.event_proxy.clear();
    cached.last_used = Instant::now();
    result
}

fn apply_turn_limits(cfg: &mut AgentConfig, limits: ChatRunLimits) {
    if let Some(max_turns) = limits.max_turns {
        cfg.max_turns = cfg.max_turns.min(max_turns);
    }
    if let Some(cell_wall_ms) = limits.cell_wall_ms {
        cfg.cell_budget.wall_ms = cfg.cell_budget.wall_ms.min(cell_wall_ms);
    }
}

#[async_trait]
impl ChatRunner for AgentChatRunner {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        let session_id = turn.session_id;
        self.shards
            .send(session_id, AgentRequest { turn, sink, reply })
            .map_err(|_| ServerError::Store("agent shard stopped".to_string()))?;
        response
            .await
            .map_err(|_| ServerError::Store("agent shard dropped response".to_string()))?
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
}

#[cfg(test)]
mod tests {
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
        StreamEvent,
    };
    use tm_host::{
        ApprovalDecision, ApprovalPolicy, FsMode, GrantDoc, HostFn, InvocationCtx,
        LinkedFolderConfig, LinkedFolders, ToolDocs,
    };

    use super::*;

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
    }

    impl Default for ReactiveLlm {
        fn default() -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                code: "increment".to_string(),
            }
        }
    }

    impl ReactiveLlm {
        fn with_code(code: impl Into<String>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                code: code.into(),
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
            let events = if request
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
            user_prompt: "advance state".to_string(),
            mode: ModeId::from("general"),
            scope: "global".to_string(),
            system_prompt: "test system".to_string(),
            capabilities: Vec::new(),
            prior_messages: Vec::new(),
            limits: ChatRunLimits::default(),
            deny_approvals: false,
            host_functions: Vec::new(),
        }
    }

    async fn run(runner: &AgentChatRunner, turn: ChatTurn) {
        let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(NullSink);
        assert_eq!(runner.run_turn(turn, sink).await.unwrap(), "done");
    }

    #[tokio::test]
    async fn tm_backend_uses_cached_session_and_forwards_structured_events() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let llm = Arc::new(ReactiveLlm::with_code(
            "[{patch: \"secret\"}] |> par map @fs.patch |> display {kind: \"table\"}",
        ));
        let llm_client: Arc<dyn LlmClient> = llm;
        let mut base_options = TmSandboxOptions {
            artifact_root: temp.path().join("artifacts"),
            ..TmSandboxOptions::default()
        };
        base_options.approval_policy = Arc::new(AlwaysApprove);
        let runner = AgentChatRunner::tm_with_options(
            llm_client,
            AgentConfig {
                model: "fake".into(),
                max_turns: 3,
                ..AgentConfig::default()
            },
            base_options,
            options(1, Duration::from_secs(60)),
        );
        let sink = Arc::new(RuntimeSink::default());
        let event_sink: Arc<dyn EventSink + Send + Sync> = sink.clone();
        let mut turn = chat_turn(Uuid::from_u128(20));
        turn.capabilities = vec!["fs.patch".into()];
        turn.host_functions = vec![Arc::new(ApprovedPatch::new(Arc::clone(&calls)))];
        assert_eq!(runner.run_turn(turn, event_sink).await.unwrap(), "done");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = sink.0.lock().unwrap();
        for expected in [
            "scope_start",
            "scope_result",
            "effect_start",
            "effect_suspended",
            "effect_resumed",
            "effect_result",
            "display",
            "binding_committed",
        ] {
            assert!(
                events.iter().any(|(event, _)| event == expected),
                "missing {expected}: {events:?}"
            );
        }
        let effect_start = events
            .iter()
            .find(|(kind, _)| kind == "effect_start")
            .unwrap();
        assert_eq!(effect_start.1["argsPreview"], "[redacted]");
    }

    #[tokio::test]
    async fn host_event_proxy_routes_only_while_a_turn_is_bound() {
        let proxy = SwappableHostEventSink::default();
        let first = Arc::new(RuntimeSink::default());
        let first_sink: Arc<dyn EventSink + Send + Sync> = first.clone();
        proxy.bind(first_sink);
        proxy
            .emit("first", serde_json::json!({"turn": 1}))
            .await
            .unwrap();

        proxy.clear();
        proxy
            .emit("between_turns", serde_json::json!({"ignored": true}))
            .await
            .unwrap();

        let second = Arc::new(RuntimeSink::default());
        let second_sink: Arc<dyn EventSink + Send + Sync> = second.clone();
        proxy.bind(second_sink);
        proxy
            .emit("second", serde_json::json!({"turn": 2}))
            .await
            .unwrap();

        assert_eq!(
            *first.0.lock().unwrap(),
            vec![("first".to_string(), serde_json::json!({"turn": 1}))]
        );
        assert_eq!(
            *second.0.lock().unwrap(),
            vec![("second".to_string(), serde_json::json!({"turn": 2}))]
        );
    }

    fn options(shard_count: usize, session_ttl: Duration) -> AgentChatRunnerOptions {
        AgentChatRunnerOptions {
            shard_count,
            session_ttl,
        }
    }

    #[tokio::test]
    async fn same_session_reuses_non_send_state() {
        let harness = Harness::new();
        let runner = harness.runner(options(2, Duration::from_secs(60)), Duration::ZERO);
        let session_id = Uuid::from_u128(1);

        run(&runner, chat_turn(session_id)).await;
        run(&runner, chat_turn(session_id)).await;

        assert_eq!(
            *harness.evaluations.lock().unwrap(),
            vec![(session_id, 1), (session_id, 2)]
        );
        assert_eq!(harness.opens.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn different_sessions_keep_isolated_state() {
        let harness = Harness::new();
        let runner = harness.runner(options(2, Duration::from_secs(60)), Duration::ZERO);
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);

        run(&runner, chat_turn(first)).await;
        run(&runner, chat_turn(second)).await;
        run(&runner, chat_turn(first)).await;

        assert_eq!(
            *harness.evaluations.lock().unwrap(),
            vec![(first, 1), (second, 1), (first, 2)]
        );
    }

    #[tokio::test]
    async fn dropping_runner_joins_workers_and_destroys_cached_sessions() {
        let harness = Harness::new();
        let runner = harness.runner(options(2, Duration::from_secs(60)), Duration::ZERO);

        run(&runner, chat_turn(Uuid::from_u128(1))).await;
        run(&runner, chat_turn(Uuid::from_u128(2))).await;
        assert_eq!(harness.drops.load(Ordering::SeqCst), 0);

        drop(runner);

        assert_eq!(harness.drops.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn deterministic_shards_serialize_same_session_turns() {
        let harness = Harness::new();
        let runner = harness.runner(
            options(3, Duration::from_secs(60)),
            Duration::from_millis(20),
        );
        let session_id = Uuid::from_u128(4);
        assert_eq!(shard_index(session_id, 3), 1);

        let (first, second) = tokio::join!(
            runner.run_turn(chat_turn(session_id), Arc::new(NullSink)),
            runner.run_turn(chat_turn(session_id), Arc::new(NullSink)),
        );
        assert_eq!(first.unwrap(), "done");
        assert_eq!(second.unwrap(), "done");
        assert_eq!(harness.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(
            *harness.evaluations.lock().unwrap(),
            vec![(session_id, 1), (session_id, 2)]
        );
        assert_eq!(harness.opens.lock().unwrap()[0].1, "tm-agent-shard-1");
    }

    #[tokio::test]
    async fn prior_messages_are_included_before_current_user_message() {
        let harness = Harness::new();
        let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
        let mut turn = chat_turn(Uuid::from_u128(7));
        turn.prior_messages = vec![
            Message::user("earlier question"),
            Message::assistant("earlier answer"),
        ];

        run(&runner, turn).await;

        let requests = harness.llm.requests.lock().unwrap();
        assert_eq!(requests[0][0].role, Role::System);
        assert!(
            requests[0][0]
                .content
                .starts_with("## Immutable tm runtime contract")
        );
        assert!(requests[0][0].content.contains("test system"));
        assert!(requests[0][0].content.contains("tm-conformance-v2"));
        assert_eq!(requests[0][1], Message::user("earlier question"));
        assert_eq!(requests[0][2], Message::assistant("earlier answer"));
        assert_eq!(requests[0][3], Message::user("advance state"));
    }

    #[tokio::test]
    async fn per_turn_limits_clamp_turn_count_and_cell_wall_budget() {
        let harness = Harness::new();
        let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
        let mut turn = chat_turn(Uuid::from_u128(8));
        turn.limits = ChatRunLimits {
            max_turns: Some(1),
            cell_wall_ms: Some(7),
        };
        let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(NullSink);

        let error = runner.run_turn(turn, sink).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("turn budget exhausted after 1 turns")
        );
        assert_eq!(*harness.wall_budgets.lock().unwrap(), vec![7]);
        assert_eq!(harness.llm.requests.lock().unwrap().len(), 1);

        let mut cfg = AgentConfig {
            max_turns: 3,
            cell_budget: CellBudget {
                wall_ms: 30,
                ..CellBudget::default()
            },
            ..AgentConfig::default()
        };
        apply_turn_limits(
            &mut cfg,
            ChatRunLimits {
                max_turns: Some(8),
                cell_wall_ms: Some(120),
            },
        );
        assert_eq!(
            cfg.max_turns, 3,
            "a turn cannot expand the base turn budget"
        );
        assert_eq!(
            cfg.cell_budget.wall_ms, 30,
            "a turn cannot expand the base cell budget"
        );
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn deny_approvals_replaces_the_base_policy() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "repo".to_string(),
            path: repo,
            mode: FsMode::Rw,
            commands: vec!["cargo".to_string()],
            safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
        }])
        .unwrap();
        let llm = Arc::new(ReactiveLlm::with_code(
            r#"let result = handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error {
  | ApprovalDeniedError {message, ...} -> {name: "ApprovalDeniedError", message: message}
  | other -> rethrow other
};
result |> display {kind: "json"}"#,
        ));
        let llm_client: Arc<dyn LlmClient> = llm.clone();
        let runner = AgentChatRunner::tm_with_options(
            llm_client,
            AgentConfig {
                model: "fake".to_string(),
                max_turns: 3,
                ..AgentConfig::default()
            },
            TmSandboxOptions {
                artifact_root: temp.path().join("artifacts"),
                linked_folders: Some(linked),
                approval_policy: Arc::new(AlwaysApprove),
                ..TmSandboxOptions::default()
            },
            options(1, Duration::from_secs(60)),
        );
        let mut turn = chat_turn(Uuid::from_u128(10));
        turn.scope = "project:repo".to_string();
        turn.capabilities = vec!["proc.*".to_string()];
        turn.deny_approvals = true;

        run(&runner, turn).await;

        let requests = llm.requests.lock().unwrap();
        let tool_result = requests[1]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn");
        assert!(
            tool_result.content.contains("ApprovalTimeoutError"),
            "tool result: {}",
            tool_result.content
        );
    }

    #[tokio::test]
    async fn expired_session_is_reopened_with_clean_state() {
        let harness = Harness::new();
        let runner = harness.runner(options(1, Duration::ZERO), Duration::ZERO);
        let session_id = Uuid::from_u128(9);
        let reset_sink = Arc::new(ResetSink::default());

        run(&runner, chat_turn(session_id)).await;
        assert_eq!(reset_sink.0.load(Ordering::SeqCst), 0);
        let mut resumed_turn = chat_turn(session_id);
        resumed_turn.prior_messages = vec![Message::user("persisted earlier turn")];
        let sink: Arc<dyn EventSink + Send + Sync> = reset_sink.clone();
        assert_eq!(runner.run_turn(resumed_turn, sink).await.unwrap(), "done");

        assert_eq!(
            *harness.evaluations.lock().unwrap(),
            vec![(session_id, 1), (session_id, 1)]
        );
        assert_eq!(harness.opens.lock().unwrap().len(), 2);
        assert_eq!(reset_sink.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn nonempty_history_without_live_state_emits_runtime_reset() {
        let harness = Harness::new();
        let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
        let reset_sink = Arc::new(ResetSink::default());
        let mut resumed_turn = chat_turn(Uuid::from_u128(11));
        resumed_turn.prior_messages = vec![
            Message::user("persisted question"),
            Message::assistant("persisted answer"),
        ];
        let sink: Arc<dyn EventSink + Send + Sync> = reset_sink.clone();

        assert_eq!(runner.run_turn(resumed_turn, sink).await.unwrap(), "done");
        assert_eq!(reset_sink.0.load(Ordering::SeqCst), 1);

        run(&runner, chat_turn(Uuid::from_u128(12))).await;
        assert_eq!(
            reset_sink.0.load(Ordering::SeqCst),
            1,
            "brand-new empty-history sessions must not emit runtime_reset"
        );
    }

    #[tokio::test]
    async fn runtime_reset_backpressure_fails_before_the_model_turn() {
        let harness = Harness::new();
        let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
        let mut resumed_turn = chat_turn(Uuid::from_u128(13));
        resumed_turn.prior_messages = vec![Message::user("persisted question")];
        let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(FailingResetSink);

        let error = runner.run_turn(resumed_turn, sink).await.unwrap_err();
        assert!(error.to_string().contains("runtime_reset backpressure"));
        assert!(harness.llm.requests.lock().unwrap().is_empty());
        assert!(harness.evaluations.lock().unwrap().is_empty());
    }
}
