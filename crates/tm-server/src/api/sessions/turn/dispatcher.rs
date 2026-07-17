use super::super::*;
use super::body::execute_turn_body;
use tokio::sync::Notify;

const TURN_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const TURN_STALE_TIMEOUT: Duration = Duration::from_secs(60);
const TURN_STALE_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
pub(super) struct TurnMaintenance {
    pub(super) heartbeat_interval: Duration,
    pub(super) stale_timeout: Duration,
    pub(super) stale_sweep_interval: Duration,
}

impl Default for TurnMaintenance {
    fn default() -> Self {
        Self {
            heartbeat_interval: TURN_HEARTBEAT_INTERVAL,
            stale_timeout: TURN_STALE_TIMEOUT,
            stale_sweep_interval: TURN_STALE_SWEEP_INTERVAL,
        }
    }
}

pub(super) async fn run_turn_dispatcher<S, M, C>(
    state: AppState<S, M, C>,
    notify: Arc<Notify>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    concurrency: usize,
    poll_interval: Duration,
    maintenance: TurnMaintenance,
) where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let worker_id = Uuid::new_v4();
    let startup = Utc::now();
    if let Err(error) = fail_stale_turns(&state, startup, maintenance.stale_timeout).await {
        let safe_error = tm_memory::redact_dream_text(&error.to_string()).text;
        tracing::error!(error = %safe_error, "could not sweep stale running turns");
        return;
    }
    let mut stale_sweep = tokio::time::interval(maintenance.stale_sweep_interval);
    stale_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    stale_sweep.tick().await;

    let max_in_flight = concurrency.max(1);
    let mut in_flight = tokio::task::JoinSet::new();
    'dispatch: loop {
        if *shutdown.borrow() {
            break;
        }
        let mut claimed_any = false;
        while in_flight.len() < max_in_flight {
            if *shutdown.borrow() {
                break 'dispatch;
            }
            let turn = match state.store.claim_next_turn(worker_id, Utc::now()).await {
                Ok(turn) => turn,
                Err(error) => {
                    let safe_error = tm_memory::redact_dream_text(&error.to_string()).text;
                    tracing::error!(error = %safe_error, "turn dispatcher claim failed");
                    break;
                }
            };
            let Some(turn) = turn else {
                break;
            };
            claimed_any = true;
            let state = state.clone();
            in_flight.spawn(async move {
                if let Err(error) =
                    execute_claimed_turn(&state, worker_id, turn, maintenance.heartbeat_interval)
                        .await
                {
                    let safe_error = tm_memory::redact_dream_text(&error.to_string()).text;
                    tracing::warn!(error = %safe_error, "durable turn failed");
                }
            });
        }

        if in_flight.len() >= max_in_flight || (!claimed_any && !in_flight.is_empty()) {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() || shutdown.has_changed().is_err() {
                        break 'dispatch;
                    }
                }
                result = in_flight.join_next() => {
                    if let Some(Err(error)) = result {
                        tracing::warn!(%error, "turn task join failed");
                    }
                }
                _ = notify.notified(), if in_flight.len() < max_in_flight => {}
                _ = stale_sweep.tick() => {
                    if let Err(error) = fail_stale_turns(&state, Utc::now(), maintenance.stale_timeout).await {
                        let safe_error = tm_memory::redact_dream_text(&error.to_string()).text;
                        tracing::warn!(error = %safe_error, "periodic stale turn sweep failed");
                    }
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        } else {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() || shutdown.has_changed().is_err() {
                        break 'dispatch;
                    }
                }
                _ = notify.notified() => {}
                _ = stale_sweep.tick() => {
                    if let Err(error) = fail_stale_turns(&state, Utc::now(), maintenance.stale_timeout).await {
                        let safe_error = tm_memory::redact_dream_text(&error.to_string()).text;
                        tracing::warn!(error = %safe_error, "periodic stale turn sweep failed");
                    }
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
    }

    // Stop claiming immediately, but let owned turns finish while the outer supervisor keeps
    // the process alive (and hard-aborts after its configured grace period).
    while let Some(result) = in_flight.join_next().await {
        if let Err(error) = result {
            tracing::warn!(%error, "turn task join failed while draining");
        }
    }
}

pub(super) async fn execute_claimed_turn<S, M, C>(
    state: &AppState<S, M, C>,
    worker_id: Uuid,
    turn: crate::SessionTurnRecord,
    heartbeat_interval: Duration,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let result =
        match execute_turn_with_heartbeat(state, worker_id, &turn, heartbeat_interval).await {
            Ok(result) => result,
            Err(error) => {
                state.runtime_status().record_heartbeat_failure();
                return Err(error);
            }
        };
    match result {
        Ok(executed) => {
            let completed = state
                .store
                .complete_turn(turn.id, worker_id, &executed.response, Utc::now())
                .await;
            if let Err(error) = completed {
                abort_turn_runtime(state, turn.session_id, turn.id, Some(executed.runtime)).await?;
                return Err(error);
            }
            if let Err(error) =
                promote_turn_runtime(state, turn.session_id, turn.id, executed.runtime).await
            {
                // The durable response is already committed. Fail closed by evicting the live
                // interpreter so the next turn reconstructs from durable history.
                abort_turn_runtime(state, turn.session_id, turn.id, Some(executed.runtime)).await?;
                return Err(error);
            }
            Ok(())
        }
        Err(error) => {
            let message = tm_memory::redact_dream_text(&error.to_string()).text;
            // Await shard cleanup before this turn is failed/reclaimed. This prevents a dropped
            // API future from leaving its std-thread interpreter mutating in the background.
            abort_turn_runtime(state, turn.session_id, turn.id, None).await?;
            state
                .store
                .fail_turn(turn.id, worker_id, &message, Utc::now())
                .await?;
            if let Ok(event) = state
                .store
                .append_event_for_turn(
                    turn.session_id,
                    "error",
                    json!({ "message": message, "turnId": turn.id }),
                    Some(turn.id),
                )
                .await
            {
                let _ = state.sender(turn.session_id).send(event);
            }
            Err(ServerError::Backend(message))
        }
    }
}

async fn execute_turn_with_heartbeat<S, M, C>(
    state: &AppState<S, M, C>,
    worker_id: Uuid,
    turn: &crate::SessionTurnRecord,
    heartbeat_interval: Duration,
) -> Result<Result<ExecutedTurn>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let execution = execute_turn_body(state, turn);
    tokio::pin!(execution);
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + heartbeat_interval,
        heartbeat_interval,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            result = &mut execution => return Ok(result),
            _ = heartbeat.tick() => {
                let heartbeat_result = state
                    .store
                    .heartbeat_turn(turn.id, worker_id, Utc::now())
                    .await;
                if let Err(error) = heartbeat_result {
                    cancel_turn_runtime(state, turn.session_id, turn.id);
                    // The concrete tm runners cooperatively terminate their cell and cross the
                    // terminal-event boundary. Keep polling the body until its shard responds,
                    // then synchronously evict any quarantined state before returning ownership.
                    let _ = execution.as_mut().await;
                    abort_turn_runtime(state, turn.session_id, turn.id, None).await?;
                    return Err(error);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TurnRuntime {
    Chat,
    Coding,
}

#[derive(Debug)]
pub(super) struct ExecutedTurn {
    pub(super) response: String,
    pub(super) runtime: TurnRuntime,
}

fn cancel_turn_runtime<S, M, C>(state: &AppState<S, M, C>, session_id: Uuid, turn_id: Uuid)
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.chat.cancel_turn(session_id, turn_id);
    if let Some(backend) = &state.coding_backend {
        backend.cancel_turn(session_id, turn_id);
    }
}

async fn promote_turn_runtime<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    turn_id: Uuid,
    runtime: TurnRuntime,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    match runtime {
        TurnRuntime::Chat => state.chat.promote_session(session_id, turn_id).await,
        TurnRuntime::Coding => {
            if let Some(backend) = &state.coding_backend {
                backend.promote_session(session_id, turn_id).await
            } else {
                Ok(())
            }
        }
    }
}

async fn abort_turn_runtime<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    turn_id: Uuid,
    runtime: Option<TurnRuntime>,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    match runtime {
        Some(TurnRuntime::Chat) => state.chat.abort_session(session_id, turn_id).await,
        Some(TurnRuntime::Coding) => {
            if let Some(backend) = &state.coding_backend {
                backend.abort_session(session_id, turn_id).await
            } else {
                Ok(())
            }
        }
        None => {
            let chat = state.chat.abort_session(session_id, turn_id).await;
            let coding = if let Some(backend) = &state.coding_backend {
                backend.abort_session(session_id, turn_id).await
            } else {
                Ok(())
            };
            chat.and(coding)
        }
    }
}

async fn fail_stale_turns<S, M, C>(
    state: &AppState<S, M, C>,
    now: chrono::DateTime<Utc>,
    stale_timeout: Duration,
) -> Result<usize>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let stale_timeout =
        chrono::Duration::from_std(stale_timeout).expect("turn stale timeout fits chrono duration");
    state
        .store
        .fail_stale_running_turns(now - stale_timeout, now, "turn owner heartbeat expired")
        .await
}
