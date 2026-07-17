use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, mpsc as std_mpsc},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_core::{
    Agent, AgentConfig, CancellationToken, EventSink, LlmClient, Sandbox, Session, SessionConfig,
};
use tm_host::{
    ApprovalDecision as HostApprovalDecision, ApprovalPolicy, CapabilityGrants,
    DefaultDenyApprovalPolicy, HostError, HostEventSink,
};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, CodingBackend, CodingEventSink,
    CodingTurn, CodingTurnResult, DetailedApprovalOutcome, Result, ServerError, StoreEvent,
    session_shards::{ThreadAffineShardPool, evict_expired_sessions, session_sweep_interval},
    turn_control::{ActiveTurnRegistry, CancellationProxy, CombinedCancellation, TurnCancellation},
};

#[cfg(test)]
use crate::session_shards::shard_index as native_shard_index;

const NATIVE_TM_BACKEND: &str = "native-tm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeApprovalMode {
    Deny,
    Manual,
}

impl NativeApprovalMode {
    pub fn parse(mode: &str) -> Result<Self> {
        match mode {
            "" | "deny" => Ok(Self::Deny),
            "manual" => Ok(Self::Manual),
            other => Err(ServerError::InvalidRequest(format!(
                "unsupported approval mode {other}"
            ))),
        }
    }
}

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
        let event_sink = Arc::new(ForwardingEventSink { sender: event_tx });
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

#[derive(Default)]
struct SwappableCodingSink {
    target: Mutex<Option<Arc<dyn CodingEventSink>>>,
}

impl SwappableCodingSink {
    fn bind(&self, sink: Arc<dyn CodingEventSink>) {
        *self.target.lock().expect("coding sink proxy lock poisoned") = Some(sink);
    }

    fn clear(&self) {
        self.target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .take();
    }
}

#[async_trait]
impl CodingEventSink for SwappableCodingSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<crate::SessionEvent> {
        let target = self
            .target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .clone()
            .ok_or_else(|| {
                ServerError::Store("native tm event sink has no active turn".to_string())
            })?;
        target.emit(event_type, payload_json).await
    }

    async fn publish_persisted(&self, event: crate::SessionEvent) -> Result<()> {
        let target = self
            .target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .clone()
            .ok_or_else(|| {
                ServerError::Store("native tm event sink has no active turn".to_string())
            })?;
        target.publish_persisted(event).await
    }

    fn turn_id(&self) -> Option<Uuid> {
        let target = self
            .target
            .lock()
            .expect("coding sink proxy lock poisoned")
            .clone();
        target.and_then(|target| target.turn_id())
    }
}

#[derive(Default)]
struct SwappableHostEventSink {
    target: Mutex<Option<Arc<dyn HostEventSink>>>,
}

impl SwappableHostEventSink {
    fn bind(&self, sink: Arc<dyn HostEventSink>) {
        *self.target.lock().expect("host sink proxy lock poisoned") = Some(sink);
    }

    fn clear(&self) {
        self.target
            .lock()
            .expect("host sink proxy lock poisoned")
            .take();
    }
}

#[async_trait]
impl HostEventSink for SwappableHostEventSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        let target = self
            .target
            .lock()
            .expect("host sink proxy lock poisoned")
            .clone()
            .ok_or_else(|| {
                HostError::HostCall("native tm host event sink has no active turn".into())
            })?;
        target.emit(event_type, payload_json).await
    }
}

pub struct HttpApprovalPolicy {
    broker: Arc<ApprovalBroker>,
    session_id: Uuid,
    sink: Arc<dyn CodingEventSink>,
    actor_id: Option<String>,
}

impl HttpApprovalPolicy {
    pub fn new(
        broker: Arc<ApprovalBroker>,
        session_id: Uuid,
        sink: Arc<dyn CodingEventSink>,
    ) -> Self {
        Self {
            broker,
            session_id,
            sink,
            actor_id: None,
        }
    }

    pub fn with_actor_id(mut self, actor_id: Option<impl Into<String>>) -> Self {
        self.actor_id = actor_id.map(Into::into);
        self
    }
}

#[async_trait]
impl ApprovalPolicy for HttpApprovalPolicy {
    async fn request(
        &self,
        action: &str,
        timeout: std::time::Duration,
    ) -> tm_host::Result<HostApprovalDecision> {
        let action = action.to_string();
        let detailed = self
            .broker
            .request_permission_detailed_for_backend(
                self.session_id,
                NATIVE_TM_BACKEND,
                approval_prompt(&action, self.actor_id.as_deref()),
                timeout,
                Arc::clone(&self.sink),
            )
            .await
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        host_decision(&action, detailed)
    }
}

fn approval_prompt(action: &str, actor_id: Option<&str>) -> ApprovalPrompt {
    let mut scope = serde_json::Map::new();
    scope.insert("action".to_string(), json!(action));
    scope.insert(
        "capability".to_string(),
        json!(action.split_whitespace().next().unwrap_or(action)),
    );
    if let Some(actor_id) = actor_id {
        scope.insert("actorId".to_string(), json!(actor_id));
    }
    ApprovalPrompt {
        action: action.to_string(),
        scope: Value::Object(scope),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject once".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn host_decision(
    action: &str,
    detailed: DetailedApprovalOutcome,
) -> tm_host::Result<HostApprovalDecision> {
    match detailed.status {
        ApprovalStatus::Approved => Ok(HostApprovalDecision::Approved),
        ApprovalStatus::Denied | ApprovalStatus::Cancelled => Ok(HostApprovalDecision::Denied),
        ApprovalStatus::TimedOut => Err(HostError::ApprovalTimeout(action.to_string())),
    }
}

enum NativeEvent {
    Reasoning(String),
    Text(String),
    ToolCall(String),
    RuntimeReset {
        persisted: Option<tokio::sync::oneshot::Sender<std::result::Result<(), String>>>,
    },
    Runtime {
        event_type: String,
        payload: Value,
        persisted: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    },
    Final(String),
}

struct ForwardingEventSink {
    sender: mpsc::Sender<NativeEvent>,
}

impl ForwardingEventSink {
    fn send(&self, event: NativeEvent) -> tm_core::Result<()> {
        self.sender.try_send(event).map_err(|error| {
            let message = match error {
                mpsc::error::TrySendError::Full(_) => {
                    "native tm bounded event channel is full".to_string()
                }
                mpsc::error::TrySendError::Closed(_) => {
                    "native tm event forwarder stopped".to_string()
                }
            };
            tm_core::Error::EventSink(message)
        })
    }
}

impl EventSink for ForwardingEventSink {
    fn on_text(&self, delta: &str) {
        let _ = self.try_on_text(delta);
    }

    fn try_on_text(&self, delta: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::Text(delta.to_string()))
    }

    fn on_reasoning(&self, delta: &str) {
        let _ = self.try_on_reasoning(delta);
    }

    fn try_on_reasoning(&self, delta: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::Reasoning(delta.to_string()))
    }

    fn on_tool_call(&self, name: &str) {
        let _ = self.try_on_tool_call(name);
    }

    fn try_on_tool_call(&self, name: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::ToolCall(name.to_string()))
    }

    // Structured tm-lang events carry the bounded/redacted cell previews. Do not forward the
    // legacy core callbacks, which contain complete source and shaped results.
    fn try_on_cell_start(&self, _code: &str) -> tm_core::Result<()> {
        Ok(())
    }

    fn try_on_cell_result(&self, _shaped: &str) -> tm_core::Result<()> {
        Ok(())
    }

    fn on_runtime_reset(&self) {
        let _ = self.try_on_runtime_reset();
    }

    fn try_on_runtime_reset(&self) -> tm_core::Result<()> {
        self.send(NativeEvent::RuntimeReset { persisted: None })
    }

    fn try_on_runtime_reset_confirmed(
        &self,
    ) -> Pin<Box<dyn Future<Output = tm_core::Result<()>> + Send + '_>> {
        Box::pin(async move {
            let (persisted, response) = tokio::sync::oneshot::channel();
            self.send(NativeEvent::RuntimeReset {
                persisted: Some(persisted),
            })?;
            response
                .await
                .map_err(|_| {
                    tm_core::Error::EventSink("native tm event forwarder stopped".to_string())
                })?
                .map_err(tm_core::Error::EventSink)
        })
    }

    fn on_final(&self, text: &str) {
        let _ = self.try_on_final(text);
    }

    fn try_on_final(&self, text: &str) -> tm_core::Result<()> {
        self.send(NativeEvent::Final(text.to_string()))
    }
}

#[async_trait]
impl HostEventSink for ForwardingEventSink {
    async fn emit(&self, event_type: &str, payload_json: Value) -> tm_host::Result<()> {
        let (persisted, response) = tokio::sync::oneshot::channel();
        self.send(NativeEvent::Runtime {
            event_type: event_type.to_string(),
            payload: payload_json,
            persisted,
        })
        .map_err(|error| HostError::HostCall(error.to_string()))?;
        response
            .await
            .map_err(|_| HostError::HostCall("native tm event forwarder stopped".into()))?
            .map_err(HostError::HostCall)
    }
}

async fn forward_events(
    mut events: mpsc::Receiver<NativeEvent>,
    sink: Arc<dyn CodingEventSink>,
) -> Result<()> {
    while let Some(event) = events.recv().await {
        let (event_type, payload): (&str, Value) = match event {
            NativeEvent::Reasoning(delta) => ("reasoning", json!({ "delta": delta })),
            NativeEvent::Text(delta) => (
                "text",
                serde_json::to_value(StoreEvent::Text { delta })
                    .map_err(|err| ServerError::Store(err.to_string()))?,
            ),
            NativeEvent::ToolCall(name) => ("tool_call", json!({ "name": name })),
            NativeEvent::RuntimeReset { persisted } => {
                match sink
                    .emit(
                        "runtime_reset",
                        json!({ "reason": "sandbox_runtime_reopened" }),
                    )
                    .await
                {
                    Ok(_) => {
                        if let Some(persisted) = persisted {
                            let _ = persisted.send(Ok(()));
                        }
                    }
                    Err(error) => {
                        if let Some(persisted) = persisted {
                            let _ = persisted.send(Err(error.to_string()));
                        }
                        return Err(error);
                    }
                }
                continue;
            }
            NativeEvent::Runtime {
                event_type,
                payload,
                persisted,
            } => {
                match sink.emit(&event_type, payload).await {
                    Ok(_) => {
                        let _ = persisted.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = persisted.send(Err(error.to_string()));
                        return Err(error);
                    }
                }
                continue;
            }
            NativeEvent::Final(text) => (
                "final",
                serde_json::to_value(StoreEvent::Final { text })
                    .map_err(|err| ServerError::Store(err.to_string()))?,
            ),
        };
        sink.emit(event_type, payload).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    use futures::stream::{self, BoxStream};
    use parking_lot::Mutex as ParkingMutex;
    use tm_core::{ChatRequest, Error as CoreError, Message, Role, StreamEvent};

    use super::*;

    struct StatefulLlm {
        requests: ParkingMutex<Vec<Vec<Message>>>,
    }

    impl StatefulLlm {
        fn new() -> Self {
            Self {
                requests: ParkingMutex::new(Vec::new()),
            }
        }

        fn tool_results(&self) -> Vec<String> {
            self.requests
                .lock()
                .iter()
                .filter_map(|messages| {
                    messages
                        .iter()
                        .find(|message| message.role == Role::Tool)
                        .map(|message| message.content.clone())
                })
                .collect()
        }
    }

    #[async_trait]
    impl LlmClient for StatefulLlm {
        async fn chat_stream(
            &self,
            request: &ChatRequest,
        ) -> tm_core::Result<BoxStream<'static, tm_core::Result<StreamEvent>>> {
            self.requests.lock().push(request.messages.clone());
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
                let code = if request.messages.iter().any(|message| {
                    message.role == Role::User && message.content == "increment native state"
                }) {
                    "let retained = retained + 1;\nretained"
                } else {
                    "let retained = 1;\nretained"
                };
                vec![
                    StreamEvent::ToolCall {
                        index: 0,
                        id: Some("call_state".to_string()),
                        name: Some("execute".to_string()),
                        arguments: Some(
                            json!({
                                "code": code
                            })
                            .to_string(),
                        ),
                    },
                    StreamEvent::Finish {
                        reason: Some("tool_calls".to_string()),
                    },
                ]
            };
            Ok(Box::pin(stream::iter(
                events.into_iter().map(Ok::<StreamEvent, CoreError>),
            )))
        }
    }

    struct BurstLlm {
        events: ParkingMutex<VecDeque<Vec<StreamEvent>>>,
    }

    impl BurstLlm {
        fn new(event_count: usize) -> Self {
            let mut events = (0..event_count)
                .map(|index| StreamEvent::Text(format!("delta-{index}")))
                .collect::<Vec<_>>();
            events.push(StreamEvent::Finish {
                reason: Some("stop".to_string()),
            });
            Self {
                events: ParkingMutex::new(VecDeque::from([events])),
            }
        }
    }

    #[async_trait]
    impl LlmClient for BurstLlm {
        async fn chat_stream(
            &self,
            _request: &ChatRequest,
        ) -> tm_core::Result<BoxStream<'static, tm_core::Result<StreamEvent>>> {
            let events = self.events.lock().pop_front().unwrap_or_default();
            Ok(Box::pin(stream::iter(
                events.into_iter().map(Ok::<StreamEvent, CoreError>),
            )))
        }
    }

    #[derive(Default)]
    struct RecordingCodingSink {
        events: ParkingMutex<Vec<(String, Value)>>,
        next_seq: AtomicI64,
        delay: Duration,
        event_to_fail: Option<&'static str>,
        event_failures: AtomicUsize,
        runtime_reset_failures: AtomicUsize,
        runtime_reset_attempts: AtomicUsize,
    }

    impl RecordingCodingSink {
        fn slow(delay: Duration) -> Self {
            Self {
                delay,
                ..Self::default()
            }
        }

        fn fail_runtime_reset_once() -> Self {
            Self {
                runtime_reset_failures: AtomicUsize::new(1),
                ..Self::default()
            }
        }

        fn fail_binding_once() -> Self {
            Self {
                event_to_fail: Some("binding_committed"),
                event_failures: AtomicUsize::new(1),
                ..Self::default()
            }
        }

        fn event_types(&self) -> Vec<String> {
            self.events
                .lock()
                .iter()
                .map(|(event_type, _)| event_type.clone())
                .collect()
        }
    }

    #[async_trait]
    impl CodingEventSink for RecordingCodingSink {
        async fn emit(&self, event_type: &str, payload_json: Value) -> Result<crate::SessionEvent> {
            let should_fail_event = self.event_to_fail == Some(event_type)
                && self
                    .event_failures
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok();
            if should_fail_event {
                return Err(ServerError::Store(format!(
                    "{event_type} persistence failed"
                )));
            }
            if event_type == "runtime_reset" {
                self.runtime_reset_attempts.fetch_add(1, Ordering::SeqCst);
                let should_fail = self
                    .runtime_reset_failures
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok();
                if should_fail {
                    return Err(ServerError::Store(
                        "runtime_reset persistence failed".to_string(),
                    ));
                }
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.events
                .lock()
                .push((event_type.to_string(), payload_json.clone()));
            Ok(crate::SessionEvent::new(
                Uuid::nil(),
                self.next_seq.fetch_add(1, Ordering::SeqCst),
                event_type,
                payload_json,
                chrono::Utc::now(),
            ))
        }
    }

    fn coding_turn(session_id: Uuid) -> CodingTurn {
        CodingTurn {
            session_id,
            durable_turn_id: None,
            user_prompt: "advance native state".to_string(),
            system_prompt: "native test system".to_string(),
            mode: tm_modes::ModeId::from("serious_engineer"),
            scope: "project:tempestmiku".to_string(),
            capabilities: Vec::new(),
            prior_messages: Vec::new(),
        }
    }

    fn backend(
        llm: Arc<dyn LlmClient>,
        artifact_root: &std::path::Path,
        options: NativeTmBackendOptions,
    ) -> NativeTmBackend {
        NativeTmBackend::new_with_options(
            llm,
            AgentConfig {
                model: "fake".to_string(),
                max_turns: 3,
                ..AgentConfig::default()
            },
            TmSandboxOptions {
                artifact_root: artifact_root.to_path_buf(),
                ..TmSandboxOptions::default()
            },
            NativeApprovalMode::Deny,
            Arc::new(ApprovalBroker::default()),
            options,
        )
    }

    fn options(session_ttl: Duration, event_channel_capacity: usize) -> NativeTmBackendOptions {
        NativeTmBackendOptions {
            shard_count: 2,
            session_ttl,
            event_channel_capacity,
        }
    }

    async fn run_turn(
        backend: &NativeTmBackend,
        turn: CodingTurn,
        sink: Arc<RecordingCodingSink>,
    ) -> Result<CodingTurnResult> {
        let sink: Arc<dyn CodingEventSink> = sink;
        backend.run_turn(turn, sink).await
    }

    #[tokio::test]
    async fn sensitive_cell_boundary_forwards_only_structured_previews() {
        let (sender, receiver) = mpsc::channel(8);
        let forwarding = Arc::new(ForwardingEventSink { sender });
        let recorded = Arc::new(RecordingCodingSink::default());
        let target: Arc<dyn CodingEventSink> = recorded.clone();
        let writer = tokio::spawn(forward_events(receiver, target));

        forwarding
            .try_on_cell_start("@fs.patch {patch: \"secret-source-value\"}")
            .unwrap();
        forwarding
            .try_on_cell_result("diff:\n+secret-result-value")
            .unwrap();
        HostEventSink::emit(
            forwarding.as_ref(),
            "cell_start",
            json!({"cellId":"cell-1","sourcePreview":"[redacted]"}),
        )
        .await
        .unwrap();
        HostEventSink::emit(
            forwarding.as_ref(),
            "cell_result",
            json!({"cellId":"cell-1","status":"completed","resultPreview":"[redacted]"}),
        )
        .await
        .unwrap();
        drop(forwarding);
        writer.await.unwrap().unwrap();

        let events = recorded.events.lock();
        assert_eq!(
            events
                .iter()
                .map(|(event_type, _)| event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["cell_start", "cell_result"]
        );
        assert_eq!(events[0].1["sourcePreview"], "[redacted]");
        assert_eq!(events[1].1["resultPreview"], "[redacted]");
        let encoded = serde_json::to_string(&*events).unwrap();
        assert!(!encoded.contains("secret-source-value"), "{encoded}");
        assert!(!encoded.contains("secret-result-value"), "{encoded}");
        assert!(!encoded.contains("\"code\""), "{encoded}");
        assert!(!encoded.contains("\"shaped\""), "{encoded}");
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn native_sessions_reuse_state_isolate_sessions_and_reset_on_profile_change() {
        let temp = tempfile::tempdir().unwrap();
        let llm = Arc::new(StatefulLlm::new());
        let llm_client: Arc<dyn LlmClient> = llm.clone();
        let backend = backend(
            llm_client,
            temp.path(),
            options(Duration::from_secs(60), 64),
        );
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert_eq!(native_shard_index(first, 2), 1);
        assert_eq!(native_shard_index(second, 2), 0);

        let first_sink = Arc::new(RecordingCodingSink::default());
        run_turn(&backend, coding_turn(first), Arc::clone(&first_sink))
            .await
            .unwrap();
        let (agent_start, binding, agent_result) = {
            let first_events = first_sink.events.lock();
            let agent_start = first_events
                .iter()
                .position(|(event, payload)| {
                    event == "cell_start" && payload.get("sourcePreview").is_some()
                })
                .unwrap();
            let binding = first_events
                .iter()
                .position(|(event, _)| event == "binding_committed")
                .unwrap();
            let agent_result = first_events
                .iter()
                .position(|(event, payload)| {
                    event == "cell_result"
                        && payload.get("status") == Some(&json!("completed"))
                        && payload.get("resultPreview").is_some()
                })
                .unwrap();
            (agent_start, binding, agent_result)
        };
        assert!(agent_start < binding && binding < agent_result);
        let mut incremented = coding_turn(first);
        incremented.user_prompt = "increment native state".to_string();
        run_turn(
            &backend,
            incremented,
            Arc::new(RecordingCodingSink::default()),
        )
        .await
        .unwrap();
        run_turn(
            &backend,
            coding_turn(second),
            Arc::new(RecordingCodingSink::default()),
        )
        .await
        .unwrap();
        let mut changed_profile = coding_turn(first);
        changed_profile.capabilities = vec!["http.get".to_string()];
        run_turn(
            &backend,
            changed_profile,
            Arc::new(RecordingCodingSink::default()),
        )
        .await
        .unwrap();

        let tool_results = llm.tool_results();
        assert_eq!(tool_results.len(), 4);
        assert!(
            tool_results[0].contains("result:\n1"),
            "{}",
            tool_results[0]
        );
        assert!(
            tool_results[1].contains("result:\n2"),
            "{}",
            tool_results[1]
        );
        assert!(
            tool_results[2].contains("result:\n1"),
            "{}",
            tool_results[2]
        );
        assert!(
            tool_results[3].contains("result:\n1"),
            "{}",
            tool_results[3]
        );
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn native_failed_runtime_event_discards_cached_session() {
        let temp = tempfile::tempdir().unwrap();
        let llm = Arc::new(StatefulLlm::new());
        let llm_client: Arc<dyn LlmClient> = llm.clone();
        let backend = backend(
            llm_client,
            temp.path(),
            options(Duration::from_secs(60), 64),
        );
        let session_id = Uuid::from_u128(15);
        let sink = Arc::new(RecordingCodingSink::fail_binding_once());

        let error = run_turn(&backend, coding_turn(session_id), Arc::clone(&sink))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("binding_committed persistence failed"),
            "{error}"
        );

        let mut retry = coding_turn(session_id);
        retry.prior_messages = vec![Message::user("persisted earlier turn")];
        run_turn(&backend, retry, Arc::clone(&sink)).await.unwrap();

        assert_eq!(sink.runtime_reset_attempts.load(Ordering::SeqCst), 1);
        assert!(sink.event_types().contains(&"runtime_reset".to_string()));
        let tool_results = llm.tool_results();
        assert_eq!(tool_results.len(), 1);
        assert!(
            tool_results[0].contains("result:\n1"),
            "{}",
            tool_results[0]
        );
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn native_durable_abort_evicts_quarantined_runtime_state() {
        let temp = tempfile::tempdir().unwrap();
        let llm = Arc::new(StatefulLlm::new());
        let llm_client: Arc<dyn LlmClient> = llm.clone();
        let backend = backend(
            llm_client,
            temp.path(),
            options(Duration::from_secs(60), 64),
        );
        let session_id = Uuid::from_u128(25);
        let committed_id = Uuid::from_u128(2501);
        let aborted_id = Uuid::from_u128(2502);
        let sink = Arc::new(RecordingCodingSink::default());

        let mut committed = coding_turn(session_id);
        committed.durable_turn_id = Some(committed_id);
        run_turn(&backend, committed, Arc::clone(&sink))
            .await
            .unwrap();
        backend
            .promote_session(session_id, committed_id)
            .await
            .unwrap();

        let mut aborted = coding_turn(session_id);
        aborted.durable_turn_id = Some(aborted_id);
        aborted.user_prompt = "increment native state".to_string();
        run_turn(&backend, aborted, Arc::clone(&sink))
            .await
            .unwrap();
        backend.abort_session(session_id, aborted_id).await.unwrap();

        run_turn(&backend, coding_turn(session_id), sink)
            .await
            .unwrap();

        let tool_results = llm.tool_results();
        assert_eq!(tool_results.len(), 3);
        assert!(tool_results[0].contains("result:\n1"));
        assert!(tool_results[1].contains("result:\n2"));
        assert!(tool_results[2].contains("result:\n1"));
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn native_ttl_reopen_emits_reset_and_includes_prior_history() {
        let temp = tempfile::tempdir().unwrap();
        let llm = Arc::new(StatefulLlm::new());
        let llm_client: Arc<dyn LlmClient> = llm.clone();
        let backend = backend(llm_client, temp.path(), options(Duration::ZERO, 64));
        let session_id = Uuid::from_u128(3);
        let first_sink = Arc::new(RecordingCodingSink::default());
        run_turn(&backend, coding_turn(session_id), Arc::clone(&first_sink))
            .await
            .unwrap();
        assert!(
            !first_sink
                .event_types()
                .contains(&"runtime_reset".to_string())
        );

        let second_sink = Arc::new(RecordingCodingSink::default());
        let mut resumed = coding_turn(session_id);
        resumed.prior_messages = vec![
            Message::user("earlier native question"),
            Message::assistant("earlier native answer"),
        ];
        run_turn(&backend, resumed, Arc::clone(&second_sink))
            .await
            .unwrap();

        assert!(
            second_sink
                .event_types()
                .contains(&"runtime_reset".to_string())
        );
        let requests = llm.requests.lock();
        assert_eq!(requests[2][0].role, Role::System);
        assert!(
            requests[2][0]
                .content
                .starts_with("## Immutable tm runtime contract")
        );
        assert!(requests[2][0].content.contains("native test system"));
        assert!(requests[2][0].content.contains("tm-conformance-v2"));
        assert_eq!(requests[2][1], Message::user("earlier native question"));
        assert_eq!(requests[2][2], Message::assistant("earlier native answer"));
        assert_eq!(requests[2][3], Message::user("advance native state"));
        drop(requests);
        let tool_results = llm.tool_results();
        assert!(tool_results[0].contains("result:\n1"));
        assert!(tool_results[1].contains("result:\n1"));
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn native_runtime_reset_persistence_failure_retries_before_model_turn() {
        let temp = tempfile::tempdir().unwrap();
        let llm = Arc::new(StatefulLlm::new());
        let llm_client: Arc<dyn LlmClient> = llm.clone();
        let backend = backend(
            llm_client,
            temp.path(),
            options(Duration::from_secs(60), 64),
        );
        let session_id = Uuid::from_u128(14);
        run_turn(
            &backend,
            coding_turn(session_id),
            Arc::new(RecordingCodingSink::default()),
        )
        .await
        .unwrap();
        let requests_before_reset = llm.requests.lock().len();

        let mut resumed = coding_turn(session_id);
        resumed.scope = "project:retry-reset".to_string();
        resumed.prior_messages = vec![
            Message::user("earlier native question"),
            Message::assistant("earlier native answer"),
        ];
        let sink = Arc::new(RecordingCodingSink::fail_runtime_reset_once());

        let error = run_turn(&backend, resumed.clone(), Arc::clone(&sink))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("runtime_reset persistence failed"),
            "{error}"
        );
        assert_eq!(llm.requests.lock().len(), requests_before_reset);

        run_turn(&backend, resumed.clone(), Arc::clone(&sink))
            .await
            .unwrap();
        let mut incremented = resumed;
        incremented.user_prompt = "increment native state".to_string();
        run_turn(&backend, incremented, Arc::clone(&sink))
            .await
            .unwrap();

        assert_eq!(sink.runtime_reset_attempts.load(Ordering::SeqCst), 2);
        let event_types = sink.event_types();
        let reset = event_types
            .iter()
            .position(|event| event == "runtime_reset")
            .unwrap();
        let first_cell = event_types
            .iter()
            .position(|event| event == "cell_start")
            .unwrap();
        assert!(
            reset < first_cell,
            "runtime reset must persist before cells"
        );
        assert_eq!(
            event_types
                .into_iter()
                .filter(|event| event == "runtime_reset")
                .count(),
            1,
            "an acknowledged runtime reset must not be persisted again"
        );
    }

    #[tokio::test]
    async fn swappable_proxy_routes_only_to_the_current_turn_sink() {
        let proxy = SwappableCodingSink::default();
        let first = Arc::new(RecordingCodingSink::default());
        let second = Arc::new(RecordingCodingSink::default());
        let first_target: Arc<dyn CodingEventSink> = first.clone();
        proxy.bind(first_target);
        proxy
            .emit("host_first", json!({ "turn": 1 }))
            .await
            .unwrap();
        let second_target: Arc<dyn CodingEventSink> = second.clone();
        proxy.bind(second_target);
        proxy
            .emit("host_second", json!({ "turn": 2 }))
            .await
            .unwrap();
        proxy.clear();

        assert_eq!(first.event_types(), vec!["host_first"]);
        assert_eq!(second.event_types(), vec!["host_second"]);
        assert!(proxy.emit("late", Value::Null).await.is_err());
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn bounded_event_forwarding_fails_the_turn_on_backpressure() {
        let temp = tempfile::tempdir().unwrap();
        let llm: Arc<dyn LlmClient> = Arc::new(BurstLlm::new(64));
        let backend = backend(llm, temp.path(), options(Duration::from_secs(60), 1));
        let sink = Arc::new(RecordingCodingSink::slow(Duration::from_millis(20)));

        let error = run_turn(&backend, coding_turn(Uuid::from_u128(4)), sink)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("native tm bounded event channel is full"),
            "{error}"
        );
    }
}
