use std::{
    collections::HashMap,
    sync::{Arc, mpsc as std_mpsc},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tm_core::{
    Agent, AgentConfig, CancellationToken, EventSink, LlmClient, Sandbox, Session, SessionConfig,
};
use tm_host::{CapabilityGrants, DefaultDenyApprovalPolicy, HostEventSink};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    approval::{HttpApprovalPolicy, NativeApprovalMode},
    events::{ForwardingEventSink, forward_events},
    sink_proxy::{SwappableCodingSink, SwappableHostEventSink},
};
use crate::{
    ApprovalBroker, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult, Result,
    ServerError,
    session_shards::{ThreadAffineShardPool, evict_expired_sessions, session_sweep_interval},
    turn_control::{ActiveTurnRegistry, CancellationProxy, CombinedCancellation, TurnCancellation},
};

struct NativeTmRequest {
    turn: CodingTurn,
    sink: Arc<dyn CodingEventSink>,
    cancellation: Arc<TurnCancellation>,
    reply: tokio::sync::oneshot::Sender<Result<CodingTurnResult>>,
}

enum NativeRequest {
    Run(NativeTmRequest),
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

struct NativeShardConfig {
    llm: Arc<dyn LlmClient>,
    agent: AgentConfig,
    sandbox: TmSandboxOptions,
    approval_mode: NativeApprovalMode,
    approval_broker: Arc<ApprovalBroker>,
    backend: NativeTmBackendOptions,
}

pub struct NativeTmBackend {
    shards: ThreadAffineShardPool<NativeRequest>,
    active_turns: ActiveTurnRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTmBackendOptions {
    pub shard_count: usize,
    pub session_ttl: Duration,
    pub event_channel_capacity: usize,
}

impl Default for NativeTmBackendOptions {
    fn default() -> Self {
        Self {
            shard_count: 4,
            session_ttl: Duration::from_secs(30 * 60),
            event_channel_capacity: 1_024,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct NativeSessionProfile {
    mode: tm_modes::ModeId,
    scope: String,
    capabilities: Vec<String>,
}

impl From<&CodingTurn> for NativeSessionProfile {
    fn from(turn: &CodingTurn) -> Self {
        Self {
            mode: turn.mode.clone(),
            scope: turn.scope.clone(),
            capabilities: turn.capabilities.clone(),
        }
    }
}

struct CachedNativeSession {
    sandbox: Arc<dyn Sandbox>,
    session: Box<dyn Session>,
    event_proxy: Arc<SwappableCodingSink>,
    host_event_proxy: Arc<SwappableHostEventSink>,
    profile: NativeSessionProfile,
    last_used: Instant,
    runtime_reset_pending: bool,
    cancellation_proxy: Arc<CancellationProxy>,
    pending_promotion: Option<Uuid>,
}

impl NativeTmBackend {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        base_options: TmSandboxOptions,
        approval_mode: NativeApprovalMode,
        approval_broker: Arc<ApprovalBroker>,
    ) -> Self {
        Self::new_with_options(
            llm,
            cfg,
            base_options,
            approval_mode,
            approval_broker,
            NativeTmBackendOptions::default(),
        )
    }

    pub fn new_with_options(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        base_options: TmSandboxOptions,
        approval_mode: NativeApprovalMode,
        approval_broker: Arc<ApprovalBroker>,
        backend_options: NativeTmBackendOptions,
    ) -> Self {
        assert!(
            backend_options.event_channel_capacity > 0,
            "event_channel_capacity must be non-zero"
        );
        let active_turns = ActiveTurnRegistry::default();
        let shards = ThreadAffineShardPool::spawn(
            backend_options.shard_count,
            "tm-native-shard",
            move |_| {
                let llm = Arc::clone(&llm);
                let cfg = cfg.clone();
                let base_options = base_options.clone();
                let approval_broker = Arc::clone(&approval_broker);
                move |receiver| {
                    run_native_shard(
                        receiver,
                        NativeShardConfig {
                            llm,
                            agent: cfg,
                            sandbox: base_options,
                            approval_mode,
                            approval_broker,
                            backend: backend_options,
                        },
                    );
                }
            },
        );
        Self {
            shards,
            active_turns,
        }
    }
}

#[async_trait]
impl CodingBackend for NativeTmBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        let (reply, response) = tokio::sync::oneshot::channel();
        let session_id = turn.session_id;
        let guard = self.active_turns.register(session_id, turn.durable_turn_id);
        let cancellation = guard.cancellation();
        self.shards
            .send(
                session_id,
                NativeRequest::Run(NativeTmRequest {
                    turn,
                    sink,
                    cancellation,
                    reply,
                }),
            )
            .map_err(|_| ServerError::Store("native coding shard stopped".to_string()))?;
        let result = response
            .await
            .map_err(|_| ServerError::Store("native coding shard dropped response".to_string()))?;
        guard.finish();
        result
    }

    async fn promote_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.shards
            .send(
                session_id,
                NativeRequest::Promote {
                    session_id,
                    turn_id,
                    reply,
                },
            )
            .map_err(|_| ServerError::Store("native coding shard stopped".to_string()))?;
        response
            .await
            .map_err(|_| ServerError::Store("native coding shard dropped promotion".to_string()))?
    }

    async fn abort_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        self.cancel_turn(session_id, turn_id);
        let (reply, response) = tokio::sync::oneshot::channel();
        self.shards
            .send(
                session_id,
                NativeRequest::Abort {
                    session_id,
                    turn_id,
                    reply,
                },
            )
            .map_err(|_| ServerError::Store("native coding shard stopped".to_string()))?;
        response
            .await
            .map_err(|_| ServerError::Store("native coding shard dropped abort".to_string()))?
    }

    fn cancel_turn(&self, session_id: Uuid, turn_id: Uuid) {
        self.active_turns.cancel(session_id, turn_id);
    }
}

fn run_native_shard(receiver: std_mpsc::Receiver<NativeRequest>, config: NativeShardConfig) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("native tm shard runtime builds");
    let sweep_interval = session_sweep_interval(config.backend.session_ttl);

    runtime.block_on(async move {
        let mut sessions = HashMap::<Uuid, CachedNativeSession>::new();
        loop {
            let request = match receiver.recv_timeout(sweep_interval) {
                Ok(request) => request,
                Err(std_mpsc::RecvTimeoutError::Timeout) => {
                    evict_expired_sessions(
                        &mut sessions,
                        config.backend.session_ttl,
                        Instant::now(),
                        |cached| cached.last_used,
                    );
                    continue;
                }
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            };
            evict_expired_sessions(
                &mut sessions,
                config.backend.session_ttl,
                Instant::now(),
                |cached| cached.last_used,
            );
            match request {
                NativeRequest::Run(request) => {
                    let result = run_cached_native_turn(
                        &request,
                        &config.llm,
                        &config.agent,
                        &config.sandbox,
                        config.approval_mode,
                        &config.approval_broker,
                        config.backend.event_channel_capacity,
                        &mut sessions,
                    )
                    .await;
                    let _ = request.reply.send(result);
                }
                NativeRequest::Promote {
                    session_id,
                    turn_id,
                    reply,
                } => {
                    let mut result = Ok(());
                    if let Some(cached) = sessions.get_mut(&session_id) {
                        match cached.pending_promotion {
                            Some(pending) if pending == turn_id => {
                                cached.pending_promotion = None;
                                cached.last_used = Instant::now();
                            }
                            Some(pending) => {
                                result = Err(ServerError::Conflict(format!(
                                    "turn {turn_id} cannot promote native runtime pending for turn {pending}"
                                )));
                            }
                            None => {}
                        }
                    }
                    let _ = reply.send(result);
                }
                NativeRequest::Abort {
                    session_id,
                    turn_id,
                    reply,
                } => {
                    if sessions
                        .get(&session_id)
                        .is_some_and(|cached| cached.pending_promotion == Some(turn_id))
                    {
                        sessions.remove(&session_id);
                    }
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_cached_native_turn(
    request: &NativeTmRequest,
    llm: &Arc<dyn LlmClient>,
    base_cfg: &AgentConfig,
    base_options: &TmSandboxOptions,
    approval_mode: NativeApprovalMode,
    approval_broker: &Arc<ApprovalBroker>,
    event_channel_capacity: usize,
    sessions: &mut HashMap<Uuid, CachedNativeSession>,
) -> Result<CodingTurnResult> {
    let session_id = request.turn.session_id;
    let profile = NativeSessionProfile::from(&request.turn);
    let reopened = sessions
        .get(&session_id)
        .is_none_or(|cached| cached.profile != profile || cached.pending_promotion.is_some());
    if reopened {
        sessions.remove(&session_id);
        let event_proxy = Arc::new(SwappableCodingSink::default());
        event_proxy.bind(Arc::clone(&request.sink));
        let proxy_sink: Arc<dyn CodingEventSink> = event_proxy.clone();
        let host_event_proxy = Arc::new(SwappableHostEventSink::default());
        let cancellation_proxy = Arc::new(CancellationProxy::new());
        cancellation_proxy.bind(Arc::clone(&request.cancellation));
        let mut options = base_options.clone();
        options.session_id = session_id.to_string();
        options.session_scope = Some(request.turn.scope.clone());
        options.grants =
            CapabilityGrants::default().allow_many(request.turn.capabilities.iter().cloned());
        options.approval_policy = match approval_mode {
            NativeApprovalMode::Deny => Arc::new(DefaultDenyApprovalPolicy),
            NativeApprovalMode::Manual => Arc::new(HttpApprovalPolicy::new(
                Arc::clone(approval_broker),
                session_id,
                Arc::clone(&proxy_sink),
            )),
        };
        options.host_event_sink = host_event_proxy.clone();
        let request_cancellation: Arc<dyn CancellationToken> = cancellation_proxy.clone();
        let cancellation: Arc<dyn CancellationToken> = match options.cancellation.take() {
            Some(application_cancellation) => Arc::new(CombinedCancellation::new(
                application_cancellation,
                request_cancellation,
            )),
            None => request_cancellation,
        };
        options.cancellation = Some(cancellation);
        let sandbox: Arc<dyn Sandbox> = Arc::new(TmSandbox::new(options));
        let session = sandbox
            .open(SessionConfig::default())
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sessions.insert(
            session_id,
            CachedNativeSession {
                sandbox,
                session,
                event_proxy,
                host_event_proxy,
                profile,
                last_used: Instant::now(),
                runtime_reset_pending: !request.turn.prior_messages.is_empty(),
                cancellation_proxy,
                pending_promotion: None,
            },
        );
    }

    let result = {
        let cached = sessions
            .get_mut(&session_id)
            .expect("native session exists after open");
        cached.event_proxy.bind(Arc::clone(&request.sink));
        cached
            .cancellation_proxy
            .bind(Arc::clone(&request.cancellation));
        let mut cfg = base_cfg.clone();
        cfg.system_prompt = request.turn.system_prompt.clone();
        let agent = Agent::new(Arc::clone(llm), Arc::clone(&cached.sandbox), cfg);
        let (event_tx, event_rx) = mpsc::channel(event_channel_capacity);
        let event_sink = Arc::new(ForwardingEventSink {
            sender: event_tx,
            effect_scope_id: request.turn.durable_turn_id.map(|id| id.to_string()),
        });
        let host_event_sink: Arc<dyn HostEventSink> = event_sink.clone();
        cached.host_event_proxy.bind(host_event_sink);
        let forwarder = tokio::spawn(forward_events(event_rx, Arc::clone(&request.sink)));
        let run_result = async {
            if cached.runtime_reset_pending {
                event_sink.try_on_runtime_reset_confirmed().await?;
                cached.runtime_reset_pending = false;
            }
            agent
                .run_with_session_and_controls(
                    &request.turn.user_prompt,
                    &request.turn.prior_messages,
                    cached.session.as_mut(),
                    event_sink.as_ref(),
                    None,
                    Some(request.cancellation.as_ref()),
                )
                .await
        }
        .await
        .map_err(|err| ServerError::Store(err.to_string()));
        cached.host_event_proxy.clear();
        cached.cancellation_proxy.clear();
        drop(event_sink);
        let forward_result = forwarder.await.map_err(|err| {
            ServerError::Store(format!("native coding event forwarder failed: {err}"))
        });
        cached.event_proxy.clear();
        cached.last_used = Instant::now();

        // A durable sink failure can close the forwarding channel while the producer is still
        // emitting. Prefer that original persistence error over the producer's secondary
        // "forwarder stopped" error so callers see the actual failed boundary.
        let result = match forward_result {
            Err(error) | Ok(Err(error)) => Err(error),
            Ok(Ok(())) => match run_result {
                Err(error) => Err(error),
                Ok(_) if request.cancellation.is_cancelled() => {
                    Err(ServerError::Store(tm_core::Error::Cancelled.to_string()))
                }
                Ok(final_text) => Ok(CodingTurnResult {
                    final_text,
                    transcript_artifact: None,
                }),
            },
        };
        cached.pending_promotion = result.as_ref().ok().and(request.turn.durable_turn_id);
        result
    };

    if result.is_err() {
        // Keep native and chat backends on the same transaction boundary: any failed turn drops
        // ephemeral state that may have advanced beyond its last confirmed durable event.
        sessions.remove(&session_id);
    }
    result
}
