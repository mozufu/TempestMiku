use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tm_core::{AgentConfig, CancellationToken, EventSink, LlmClient, Sandbox, Session};
use tm_host::{CapabilityGrants, DefaultDenyApprovalPolicy, HostEventSink};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_modes::ModeId;
use uuid::Uuid;

use crate::{
    Result, ServerError,
    session_shards::ThreadAffineShardPool,
    turn_control::{ActiveTurnRegistry, CancellationProxy, TurnCancellation},
};

mod contract;
mod dispatch;
mod shard;

pub use contract::{ChatRunLimits, ChatRunner, ChatTurn, DialecticTurn};
pub use dispatch::{EchoChatRunner, ServerChatRunner};

use shard::run_agent_shard;

struct AgentTurnRequest {
    turn: ChatTurn,
    sink: Arc<dyn EventSink + Send + Sync>,
    cancellation: Arc<TurnCancellation>,
    reply: tokio::sync::oneshot::Sender<Result<String>>,
}

enum AgentRequest {
    Run(Box<AgentTurnRequest>),
    Promote {
        session_id: Uuid,
        turn_id: Uuid,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    Abort {
        session_id: Uuid,
        turn_id: Uuid,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
}

pub struct AgentChatRunner {
    shards: ThreadAffineShardPool<AgentRequest>,
    active_turns: ActiveTurnRegistry,
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

type SandboxFactory = dyn Fn(&ChatTurn, Arc<dyn HostEventSink>, Arc<dyn CancellationToken>) -> Arc<dyn Sandbox>
    + Send
    + Sync
    + 'static;

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
    cancellation_proxy: Arc<CancellationProxy>,
    runtime_reset_pending: bool,
    pending_promotion: Option<Uuid>,
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
        let sink = sink.ok_or_else(|| {
            tm_host::HostError::HostCall(
                "runtime event emitted without an active turn sink".to_string(),
            )
        })?;
        sink.try_on_runtime_event_confirmed(event_type, &payload_json)
            .await
            .map_err(|error| tm_host::HostError::HostCall(error.to_string()))?;
        Ok(())
    }

    fn effect_scope_id(&self) -> Option<String> {
        self.sink
            .lock()
            .expect("host event proxy lock poisoned")
            .as_ref()
            .and_then(|sink| sink.effect_scope_id())
    }
}

impl AgentChatRunner {
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
            move |turn, host_events, cancellation| {
                let mut options = base_options.clone();
                options.session_id = turn.session_id.to_string();
                options.session_scope = Some(turn.scope.clone());
                options.grants =
                    CapabilityGrants::default().allow_many(turn.capabilities.iter().cloned());
                for function in &turn.host_functions {
                    options.host_registry.register(Arc::clone(function));
                }
                if turn.deny_approvals {
                    options.approval_policy = Arc::new(DefaultDenyApprovalPolicy);
                }
                options.host_event_sink = host_events;
                options.cancellation = Some(cancellation);
                Arc::new(TmSandbox::new(options))
            },
        )
    }

    #[cfg(test)]
    fn new_with_sandbox_factory_and_options(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        runner_options: AgentChatRunnerOptions,
        sandbox_factory: impl Fn(&ChatTurn) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_host_event_sandbox_factory_and_options(
            llm,
            cfg,
            runner_options,
            move |turn, _host_events, _cancellation| sandbox_factory(turn),
        )
    }

    fn new_with_host_event_sandbox_factory_and_options(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        runner_options: AgentChatRunnerOptions,
        sandbox_factory: impl Fn(
            &ChatTurn,
            Arc<dyn HostEventSink>,
            Arc<dyn CancellationToken>,
        ) -> Arc<dyn Sandbox>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let sandbox_factory: Arc<SandboxFactory> = Arc::new(sandbox_factory);
        let active_turns = ActiveTurnRegistry::default();
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
        Self {
            shards,
            active_turns,
        }
    }
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
        let guard = self.active_turns.register(session_id, turn.durable_turn_id);
        let cancellation = guard.cancellation();
        self.shards
            .send(
                session_id,
                AgentRequest::Run(Box::new(AgentTurnRequest {
                    turn,
                    sink,
                    cancellation,
                    reply,
                })),
            )
            .map_err(|_| ServerError::Store("agent shard stopped".to_string()))?;
        let result = response
            .await
            .map_err(|_| ServerError::Store("agent shard dropped response".to_string()))?;
        guard.finish();
        result
    }

    async fn promote_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.shards
            .send(
                session_id,
                AgentRequest::Promote {
                    session_id,
                    turn_id,
                    reply,
                },
            )
            .map_err(|_| ServerError::Store("agent shard stopped".to_string()))?;
        response
            .await
            .map_err(|_| ServerError::Store("agent shard dropped promotion".to_string()))?
    }

    async fn abort_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        self.cancel_turn(session_id, turn_id);
        let (reply, response) = tokio::sync::oneshot::channel();
        self.shards
            .send(
                session_id,
                AgentRequest::Abort {
                    session_id,
                    turn_id,
                    reply,
                },
            )
            .map_err(|_| ServerError::Store("agent shard stopped".to_string()))?;
        response
            .await
            .map_err(|_| ServerError::Store("agent shard dropped abort".to_string()))?
    }

    fn cancel_turn(&self, session_id: Uuid, turn_id: Uuid) {
        self.active_turns.cancel(session_id, turn_id);
    }
}

#[cfg(test)]
mod tests;
