use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use tm_core::{Agent, AgentConfig, CancellationToken, LlmClient};
use tm_host::HostEventSink;
use uuid::Uuid;

use crate::{
    Result, ServerError,
    session_shards::{evict_expired_sessions, session_sweep_interval},
    turn_control::CancellationProxy,
};

use super::{
    AgentRequest, AgentTurnRequest, CachedSession, SandboxFactory, SandboxProfile,
    SwappableHostEventSink, apply_turn_limits,
};

pub(super) fn run_agent_shard(
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
        match request {
            AgentRequest::Run(request) => {
                let result = runtime.block_on(run_cached_turn(
                    &request,
                    &llm,
                    &cfg,
                    sandbox_factory.as_ref(),
                    &mut sessions,
                ));
                let _ = request.reply.send(result);
            }
            AgentRequest::Promote {
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
                                "turn {turn_id} cannot promote runtime pending for turn {pending}"
                            )));
                        }
                        None => {}
                    }
                }
                let _ = reply.send(result);
            }
            AgentRequest::Abort {
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
}

async fn run_cached_turn(
    request: &AgentTurnRequest,
    llm: &Arc<dyn LlmClient>,
    base_cfg: &AgentConfig,
    sandbox_factory: &SandboxFactory,
    sessions: &mut HashMap<Uuid, CachedSession>,
) -> Result<String> {
    let session_id = request.turn.session_id;
    let profile = SandboxProfile::from(&request.turn);
    let replace_session = sessions
        .get(&session_id)
        .is_none_or(|cached| cached.profile != profile || cached.pending_promotion.is_some());
    if replace_session {
        sessions.remove(&session_id);
        let event_proxy = Arc::new(SwappableHostEventSink::default());
        event_proxy.bind(Arc::clone(&request.sink));
        let host_events: Arc<dyn HostEventSink> = event_proxy.clone();
        let cancellation_proxy = Arc::new(CancellationProxy::new());
        cancellation_proxy.bind(Arc::clone(&request.cancellation));
        let cancellation: Arc<dyn CancellationToken> = cancellation_proxy.clone();
        let sandbox = sandbox_factory(&request.turn, host_events, cancellation);
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
                cancellation_proxy,
                runtime_reset_pending: !request.turn.prior_messages.is_empty(),
                pending_promotion: None,
            },
        );
    }

    let result = {
        let cached = sessions
            .get_mut(&session_id)
            .expect("cached session exists after open");
        cached.event_proxy.bind(Arc::clone(&request.sink));
        cached
            .cancellation_proxy
            .bind(Arc::clone(&request.cancellation));
        let mut cfg = base_cfg.clone();
        cfg.system_prompt = request.turn.system_prompt.clone();
        apply_turn_limits(&mut cfg, request.turn.limits);
        let agent = Agent::new(Arc::clone(llm), Arc::clone(&cached.sandbox), cfg);
        let result = async {
            if cached.runtime_reset_pending {
                request.sink.try_on_runtime_reset_confirmed().await?;
                cached.runtime_reset_pending = false;
            }
            let response = agent
                .run_with_session_and_controls(
                    &request.turn.user_prompt,
                    &request.turn.prior_messages,
                    cached.session.as_mut(),
                    request.sink.as_ref(),
                    None,
                    Some(request.cancellation.as_ref()),
                )
                .await?;
            // Runtime state is reusable only after every earlier streaming/final callback has
            // crossed the sink's durable boundary. This non-closing barrier lets the API perform
            // its later ownership/flush step without leaving a failed turn cached in the meantime.
            request.sink.try_on_event_barrier_confirmed().await?;
            if request.cancellation.is_cancelled() {
                return Err(tm_core::Error::Cancelled);
            }
            Ok::<String, tm_core::Error>(response)
        }
        .await
        .map_err(|err| ServerError::Store(err.to_string()));
        cached.event_proxy.clear();
        cached.cancellation_proxy.clear();
        cached.last_used = Instant::now();
        cached.pending_promotion = result.as_ref().ok().and(request.turn.durable_turn_id);
        result
    };

    if result.is_err() {
        // A failed turn can leave a backend-specific session between an in-memory mutation and
        // its durable event. Reopening is the conservative boundary: persisted conversation
        // history survives, while unconfirmed ephemeral runtime state cannot leak into a retry.
        sessions.remove(&session_id);
    }
    result
}
