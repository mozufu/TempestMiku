use super::*;
use sha2::{Digest, Sha256};
use tm_core::Message;
use tm_host::HostFn;
use tokio::sync::Notify;

const TURN_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const TURN_STALE_TIMEOUT: Duration = Duration::from_secs(60);
const TURN_STALE_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
struct TurnMaintenance {
    heartbeat_interval: Duration,
    stale_timeout: Duration,
    stale_sweep_interval: Duration,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostMessageRequest {
    pub client_message_id: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageResponse {
    pub turn_id: Uuid,
    pub client_message_id: String,
    pub status: String,
}

pub(crate) async fn post_message<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<PostMessageRequest>,
) -> Result<impl IntoResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    validate_client_message_id(&payload.client_message_id)?;
    validate_message_content(&payload.content)?;
    let turn = state
        .store
        .enqueue_turn(session_id, &payload.client_message_id, &payload.content)
        .await?;
    state.notify_turn_dispatcher();
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(PostMessageResponse {
            turn_id: turn.id,
            client_message_id: turn.client_message_id,
            status: turn.status,
        }),
    ))
}

pub(crate) async fn get_turn<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, turn_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<crate::SessionTurnRecord>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let turn = state.store.turn(turn_id).await?;
    if turn.session_id != session_id {
        return Err(ServerError::NotFound(format!("turn {turn_id}")));
    }
    Ok(Json(turn))
}

pub(crate) fn start_turn_dispatcher<S, M, C>(state: AppState<S, M, C>, notify: Arc<Notify>)
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let (shutdown_guard, shutdown) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        // The app-level dispatcher exists only for deterministic in-process tests. Keeping
        // the sender alive prevents the watch channel from looking like a shutdown request.
        let _shutdown_guard = shutdown_guard;
        run_turn_dispatcher(
            state,
            notify,
            shutdown,
            4,
            Duration::from_millis(250),
            TurnMaintenance::default(),
        )
        .await;
    });
}

pub(crate) fn start_supervised_turn_dispatcher<S, M, C>(
    state: AppState<S, M, C>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    concurrency: usize,
    poll_interval: Duration,
) -> tokio::task::JoinHandle<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let notify = Arc::clone(&state.turn_notify);
    tokio::spawn(run_turn_dispatcher(
        state,
        notify,
        shutdown,
        concurrency,
        poll_interval,
        TurnMaintenance::default(),
    ))
}

async fn run_turn_dispatcher<S, M, C>(
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

async fn execute_claimed_turn<S, M, C>(
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
enum TurnRuntime {
    Chat,
    Coding,
}

#[derive(Debug)]
struct ExecutedTurn {
    response: String,
    runtime: TurnRuntime,
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

async fn execute_turn_body<S, M, C>(
    state: &AppState<S, M, C>,
    durable_turn: &crate::SessionTurnRecord,
) -> Result<ExecutedTurn>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session_id = durable_turn.session_id;
    let session = state.store.get_session(session_id).await?;
    if session.status == "ended" {
        return Err(ServerError::Conflict(format!(
            "session {session_id} has ended"
        )));
    }
    resources::util::validate_authorized_memory_scope(
        &state.linked_folders,
        &session.memory_scope,
    )?;
    let prior_messages =
        bounded_prior_messages(state.store.as_ref(), session_id, durable_turn.id).await?;
    // Mode changes are never inferred from keywords. An unlocked normal chat turn receives the
    // exact modes.suggest grant and may call it through execute; coding, actor, scheduler, and
    // locked turns do not receive that authority.
    let turn_profile = mode_profile(&state.persona, &session.mode_state.mode);
    let scope = session.memory_scope.clone();
    let subject = session.owner_subject.clone();
    let persona_prompt = build_turn_prompt(state, &session.mode_state.mode, &durable_turn.content);
    state
        .store
        .ensure_memory_scope_active(&subject, &scope)
        .await?;
    let memory = if let Some(event) = state
        .store
        .event_for_turn(session_id, durable_turn.id, "memory_recall")
        .await?
    {
        persisted_memory_context(&event, &subject, &scope)?
    } else {
        let memory = state
            .memory
            .context_for_turn(&subject, &scope, &durable_turn.content)
            .await?;
        if memory.requires_durable_trace() {
            let (event, inserted) = state
                .store
                .append_event_for_turn_once(
                    session_id,
                    "memory_recall",
                    json!({
                        "schemaVersion": 1,
                        "resourceUri": format!("memory://recalls/{}", durable_turn.id),
                        "context": &memory,
                    }),
                    durable_turn.id,
                )
                .await?;
            if inserted {
                let _ = state.sender(session_id).send(event.clone());
            }
            persisted_memory_context(&event, &subject, &scope)?
        } else {
            memory
        }
    };
    let mut recall_blocks = Vec::new();
    if !memory.is_empty() {
        recall_blocks.push(memory.render_prompt_block());
    }
    if let Some(block) = drive_recall_block(&state.drive_store, &scope).await {
        recall_blocks.push(block);
    }
    let user_prompt = if recall_blocks.is_empty() {
        durable_turn.content.clone()
    } else {
        format!(
            "{}\n\n[recall]\n{}",
            durable_turn.content,
            recall_blocks.join("\n\n")
        )
    };
    let mode_suggest_host: Arc<dyn HostFn> =
        Arc::new(ModeSuggestHostFn::new(state, MODE_SUGGEST_APPROVAL_TIMEOUT));
    let mut chat_capabilities = turn_profile.capabilities.clone();
    if session.mode_state.lock_source.is_none() && !turn_profile.has_capability("backend.coding") {
        chat_capabilities.push(MODE_SUGGEST_CAPABILITY.to_string());
    }
    // Registering a handler is deliberately independent from granting it. Cached sessions may
    // retain this stable, server-owned service, but HostRegistry still rejects calls unless the
    // current sandbox profile includes modes.suggest.
    let chat_host_functions = vec![mode_suggest_host];

    let (response, runtime) = if turn_profile.has_capability("backend.coding") {
        if let Some(backend) = &state.coding_backend {
            let sink = Arc::new(StoreCodingEventSink::for_turn(
                session_id,
                durable_turn.id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            let response = backend
                .run_turn(
                    CodingTurn {
                        session_id,
                        durable_turn_id: Some(durable_turn.id),
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode.clone(),
                        scope: scope.clone(),
                        capabilities: turn_profile.capabilities.clone(),
                        prior_messages: prior_messages.clone(),
                    },
                    sink,
                )
                .await?
                .final_text;
            (response, TurnRuntime::Coding)
        } else {
            let sink = Arc::new(PersistingEventSink::for_turn(
                session_id,
                Some(durable_turn.id),
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            let response = state
                .chat
                .run_turn(
                    ChatTurn {
                        session_id,
                        durable_turn_id: Some(durable_turn.id),
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode.clone(),
                        scope: scope.clone(),
                        capabilities: chat_capabilities.clone(),
                        prior_messages: prior_messages.clone(),
                        limits: crate::ChatRunLimits::default(),
                        deny_approvals: false,
                        host_functions: chat_host_functions.clone(),
                    },
                    sink.clone(),
                )
                .await;
            let flushed = sink.flush().await;
            let response = match (response, flushed) {
                (Ok(response), Ok(())) => response,
                (Err(error), _) => return Err(error),
                (Ok(_), Err(error)) => return Err(error),
            };
            (response, TurnRuntime::Chat)
        }
    } else {
        let sink = Arc::new(PersistingEventSink::for_turn(
            session_id,
            Some(durable_turn.id),
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        let response = state
            .chat
            .run_turn(
                ChatTurn {
                    session_id,
                    durable_turn_id: Some(durable_turn.id),
                    user_prompt,
                    system_prompt: persona_prompt.system_prompt,
                    mode: session.mode_state.mode.clone(),
                    scope: scope.clone(),
                    capabilities: chat_capabilities,
                    prior_messages,
                    limits: crate::ChatRunLimits::default(),
                    deny_approvals: false,
                    host_functions: chat_host_functions,
                },
                sink.clone(),
            )
            .await;
        let flushed = sink.flush().await;
        let response = match (response, flushed) {
            (Ok(response), Ok(())) => response,
            (Err(error), _) => return Err(error),
            (Ok(_), Err(error)) => return Err(error),
        };
        (response, TurnRuntime::Chat)
    };

    persist_drive_recall_chunks(state, &scope).await?;
    if turn_profile.has_capability("backend.coding") && !response.trim().is_empty() {
        state
            .store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: scope.clone(),
                text: format!(
                    "Project summary/open loop from session {session_id}: {}",
                    response.trim()
                ),
                source: format!("session:{session_id}:assistant"),
                importance: 0.58,
                created_at: Utc::now(),
            })
            .await?;
        record_project_observations(
            state,
            session_id,
            project_id_from_scope(&scope),
            &durable_turn.content,
            &response,
        )
        .await?;
    }
    super::memory_write::spawn_personal_assistant_state_capture(
        state.clone(),
        session_id,
        turn_profile,
        subject,
        scope,
        durable_turn.content.clone(),
    )
    .await?;
    Ok(ExecutedTurn { response, runtime })
}

fn persisted_memory_context(
    event: &crate::SessionEvent,
    subject: &str,
    scope: &str,
) -> Result<crate::MemoryContext> {
    let memory: crate::MemoryContext =
        serde_json::from_value(event.payload_json.get("context").cloned().ok_or_else(|| {
            ServerError::Store("persisted memory recall is missing its context".to_string())
        })?)
        .map_err(|error| {
            ServerError::Store(format!("persisted memory recall is invalid: {error}"))
        })?;
    if memory.subject != subject || memory.scope != scope {
        return Err(ServerError::Store(
            "persisted memory recall authority does not match the durable turn".to_string(),
        ));
    }
    Ok(memory)
}

async fn bounded_prior_messages<S: Store>(
    store: &S,
    session_id: Uuid,
    current_turn_id: Uuid,
) -> Result<Vec<Message>> {
    const MAX_MESSAGES: usize = 40;
    const MAX_BYTES: usize = 128 * 1024;

    let stored = store.session_messages(session_id).await?;
    let current_seq = stored
        .iter()
        .find(|message| message.turn_id == Some(current_turn_id) && message.role == "user")
        .map(|message| message.seq)
        .ok_or_else(|| {
            ServerError::Store(format!(
                "turn {current_turn_id} is missing its persisted user message"
            ))
        })?;
    let mut linked_roles = std::collections::BTreeMap::<Uuid, (bool, bool)>::new();
    for message in stored.iter().filter(|message| message.seq < current_seq) {
        let Some(turn_id) = message.turn_id else {
            continue;
        };
        let roles = linked_roles.entry(turn_id).or_default();
        match message.role.as_str() {
            "user" => roles.0 = true,
            "assistant" => roles.1 = true,
            _ => {}
        }
    }
    let complete_turns = linked_roles
        .into_iter()
        .filter_map(|(turn_id, (has_user, has_assistant))| {
            (has_user && has_assistant).then_some(turn_id)
        })
        .collect::<std::collections::BTreeSet<_>>();

    let mut bytes = 0usize;
    let mut newest = Vec::new();
    for message in stored
        .into_iter()
        .filter(|message| {
            message.seq < current_seq
                && message
                    .turn_id
                    .is_none_or(|turn_id| complete_turns.contains(&turn_id))
        })
        .rev()
    {
        if newest.len() == MAX_MESSAGES {
            break;
        }
        let next = bytes.saturating_add(message.content.len());
        if next > MAX_BYTES {
            break;
        }
        let message = match message.role.as_str() {
            "user" => Message::user(message.content),
            "assistant" => Message::assistant(message.content),
            _ => continue,
        };
        bytes = next;
        newest.push(message);
    }
    newest.reverse();
    Ok(newest)
}

fn validate_client_message_id(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if valid {
        Ok(())
    } else {
        Err(ServerError::InvalidRequest(
            "clientMessageId must be 1-128 safe ASCII characters".to_string(),
        ))
    }
}

fn validate_message_content(value: &str) -> Result<()> {
    const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
    if value.trim().is_empty() {
        return Err(ServerError::InvalidRequest(
            "message content must not be empty".to_string(),
        ));
    }
    if value.len() > MAX_MESSAGE_BYTES {
        return Err(ServerError::InvalidRequest(format!(
            "message content exceeds {MAX_MESSAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

async fn drive_recall_block(
    drive_store: &Option<tm_drive::SharedDriveStore>,
    scope: &str,
) -> Option<String> {
    let store = drive_store.as_ref()?;
    let project = drive_project_from_scope(scope)?;
    let hits = store
        .search(tm_drive::DriveSearchOptions {
            project: Some(project),
            limit: 3,
            return_snippets: true,
            ..tm_drive::DriveSearchOptions::default()
        })
        .await
        .ok()?;
    if hits.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    lines.push(format!(
        "Drive recall (scope: {scope}; included docs: {}; raw content remains behind drive:// resources).",
        hits.len()
    ));
    for hit in hits {
        let title = hit.title.as_deref().unwrap_or(&hit.path);
        let snippet = hit
            .snippet
            .unwrap_or_else(|| "No summary available.".to_string());
        lines.push(format!(
            "- [{}; hash: {}; selector: {}] {} -- {}",
            hit.uri,
            hit.content_hash,
            hit.selector.as_deref().unwrap_or("preview"),
            title,
            snippet.replace('\n', " ")
        ));
    }
    Some(lines.join("\n"))
}

async fn persist_drive_recall_chunks<S, M, C>(
    state: &AppState<S, M, C>,
    scope: &str,
) -> Result<usize>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let Some(store) = state.drive_store.as_ref() else {
        return Ok(0);
    };
    let Some(project) = drive_project_from_scope(scope) else {
        return Ok(0);
    };
    let hits = store
        .search(tm_drive::DriveSearchOptions {
            project: Some(project),
            limit: 20,
            return_snippets: true,
            ..tm_drive::DriveSearchOptions::default()
        })
        .await
        .map_err(|err| ServerError::Store(err.to_string()))?;
    let mut persisted = 0;
    for hit in hits {
        let entry = store
            .list(tm_drive::DriveListOptions {
                path: Some(hit.path.clone()),
                recursive: true,
                limit: 1,
                include_archived: true,
            })
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .into_iter()
            .find(|entry| entry.path == hit.path)
            .ok_or_else(|| ServerError::NotFound(hit.uri.clone()))?;
        let chunk = drive_entry_recall_chunk(scope, &entry);
        state.store.upsert_recall_chunk(chunk).await?;
        persisted += 1;
    }
    Ok(persisted)
}

fn drive_project_from_scope(scope: &str) -> Option<String> {
    scope
        .strip_prefix("project:")
        .filter(|project| !project.trim().is_empty())
        .map(str::to_string)
}

fn drive_entry_recall_chunk(scope: &str, entry: &tm_drive::DriveEntry) -> RecallChunkRecord {
    let title = entry.title.as_deref().unwrap_or(&entry.path);
    let summary = entry.summary.as_deref().unwrap_or("No summary available.");
    let provenance = entry.provenance.last();
    let extractor = provenance
        .map(|item| item.extractor.as_str())
        .or_else(|| entry.attributes.first().map(|attr| attr.extractor.as_str()))
        .unwrap_or("unknown");
    let source_session = provenance
        .and_then(|item| item.session_id.as_deref())
        .unwrap_or("unknown");
    let source_event = provenance
        .and_then(|item| item.event_seq)
        .map(|seq| seq.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    RecallChunkRecord {
        id: stable_drive_recall_id(scope, &entry.content_hash),
        scope: scope.to_string(),
        text: format!(
            "Drive document: {title}\nURI: {}\nContent hash: {}\nExtractor: {extractor}\nDoc kind: {}\nTags: {}\nAttributes: {}\nSummary: {}",
            entry.uri,
            entry.content_hash,
            entry.doc_kind.as_deref().unwrap_or("unknown"),
            drive_recall_tags(entry),
            drive_recall_attributes(entry),
            summary.replace('\n', " ")
        ),
        source: format!(
            "drive:{};content_hash:{};extractor:{};source_session:{};source_event:{}",
            entry.uri, entry.content_hash, extractor, source_session, source_event
        ),
        importance: 0.57,
        created_at: entry.updated_at,
    }
}

fn drive_recall_tags(entry: &tm_drive::DriveEntry) -> String {
    if entry.tags.is_empty() {
        "none".to_string()
    } else {
        entry.tags.join(", ")
    }
}

fn drive_recall_attributes(entry: &tm_drive::DriveEntry) -> String {
    let attributes = entry
        .attributes
        .iter()
        .filter(|attr| !matches!(attr.key.as_str(), "summary" | "content_hash"))
        .take(8)
        .map(|attr| format!("{}={} ({:.2})", attr.key, attr.value, attr.confidence))
        .collect::<Vec<_>>();
    if attributes.is_empty() {
        "none".to_string()
    } else {
        attributes.join("; ")
    }
}

fn stable_drive_recall_id(scope: &str, content_hash: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"tempestmiku:drive-recall:v1\0");
    hasher.update(scope.as_bytes());
    hasher.update(b"\0");
    hasher.update(content_hash.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod dispatcher_tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use tm_core::EventSink;
    use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};

    use super::*;
    use crate::{
        AuthConfig, ChatTurn, EchoChatRunner, InMemoryStore, MemoryContext, NewSession,
        RecallChunkRecord, StoreMemoryProvider,
    };

    #[derive(Default)]
    struct ConcurrentRunner {
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    struct SlowRunner;

    struct ReplayMustNotQueryMemory;

    #[derive(Clone, Copy)]
    enum OwnershipLossPoint {
        BeforeComplete,
        DuringHeartbeat,
    }

    struct LinearizedRunner {
        store: Arc<InMemoryStore>,
        loss: OwnershipLossPoint,
        losses_remaining: AtomicUsize,
        committed: AtomicUsize,
        pending: Mutex<Option<(Uuid, Uuid, usize)>>,
        observed: Mutex<Vec<usize>>,
        cancelled: AtomicBool,
        cancellation: tokio::sync::Notify,
        promotions: AtomicUsize,
        aborts: AtomicUsize,
    }

    impl LinearizedRunner {
        fn new(store: Arc<InMemoryStore>, loss: OwnershipLossPoint) -> Self {
            Self {
                store,
                loss,
                losses_remaining: AtomicUsize::new(1),
                committed: AtomicUsize::new(0),
                pending: Mutex::new(None),
                observed: Mutex::new(Vec::new()),
                cancelled: AtomicBool::new(false),
                cancellation: tokio::sync::Notify::new(),
                promotions: AtomicUsize::new(0),
                aborts: AtomicUsize::new(0),
            }
        }

        fn take_loss(&self) -> bool {
            self.losses_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
        }
    }

    #[async_trait]
    impl MemoryProvider for ReplayMustNotQueryMemory {
        async fn context_for_turn(
            &self,
            _subject: &str,
            _scope: &str,
            _query: &str,
        ) -> Result<MemoryContext> {
            panic!("a durable memory_recall event must be replayed without re-querying memory")
        }
    }

    #[async_trait]
    impl ChatRunner for ConcurrentRunner {
        async fn run_turn(
            &self,
            _turn: ChatTurn,
            _sink: Arc<dyn EventSink + Send + Sync>,
        ) -> Result<String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(75)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok("done".to_string())
        }
    }

    #[async_trait]
    impl ChatRunner for SlowRunner {
        async fn run_turn(
            &self,
            _turn: ChatTurn,
            _sink: Arc<dyn EventSink + Send + Sync>,
        ) -> Result<String> {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok("slow turn done".to_string())
        }
    }

    #[async_trait]
    impl ChatRunner for LinearizedRunner {
        async fn run_turn(
            &self,
            turn: ChatTurn,
            _sink: Arc<dyn EventSink + Send + Sync>,
        ) -> Result<String> {
            let turn_id = turn
                .durable_turn_id
                .expect("linearization fixture only runs durable turns");
            self.cancelled.store(false, Ordering::SeqCst);
            let next = self.committed.load(Ordering::SeqCst) + 1;
            self.observed.lock().unwrap().push(next);
            *self.pending.lock().unwrap() = Some((turn.session_id, turn_id, next));

            if self.take_loss() {
                self.store
                    .fail_stale_running_turns(
                        Utc::now() + chrono::Duration::seconds(1),
                        Utc::now(),
                        "fixture ownership loss",
                    )
                    .await?;
                if matches!(self.loss, OwnershipLossPoint::DuringHeartbeat) {
                    while !self.cancelled.load(Ordering::SeqCst) {
                        self.cancellation.notified().await;
                    }
                    return Err(ServerError::Backend("fixture cancelled".to_string()));
                }
            }

            Ok(format!("state {next}"))
        }

        async fn promote_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
            let pending = self.pending.lock().unwrap().take();
            if let Some((pending_session, pending_turn, value)) = pending {
                if pending_session != session_id || pending_turn != turn_id {
                    return Err(ServerError::Conflict(
                        "fixture promotion identity mismatch".to_string(),
                    ));
                }
                self.committed.store(value, Ordering::SeqCst);
                self.promotions.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn abort_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
            let mut pending = self.pending.lock().unwrap();
            if pending
                .as_ref()
                .is_some_and(|(pending_session, pending_turn, _)| {
                    *pending_session == session_id && *pending_turn == turn_id
                })
            {
                pending.take();
            }
            self.aborts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn cancel_turn(&self, _session_id: Uuid, _turn_id: Uuid) {
            self.cancelled.store(true, Ordering::SeqCst);
            self.cancellation.notify_one();
        }
    }

    #[tokio::test]
    async fn durable_memory_recall_event_is_the_exact_retry_context() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, "memory-retry", "retry the durable turn")
            .await
            .unwrap();
        let memory = MemoryContext::from_records(
            "owner",
            "global",
            Vec::new(),
            vec![RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "global".to_string(),
                text: "persisted recall survives the retry boundary".to_string(),
                source: "memory://fixture/retry".to_string(),
                importance: 0.8,
                created_at: Utc::now(),
            }],
            1_600,
        )
        .mark_lexical_fallback("fixture_restart");
        store
            .append_event_for_turn(
                session.id,
                "memory_recall",
                json!({
                    "schemaVersion": 1,
                    "resourceUri": format!("memory://recalls/{}", turn.id),
                    "context": memory,
                }),
                Some(turn.id),
            )
            .await
            .unwrap();
        let state = AppState::new(
            Arc::clone(&store),
            Arc::new(ReplayMustNotQueryMemory),
            Arc::new(EchoChatRunner),
            persona,
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(false);

        let output = execute_turn_body(&state, &turn).await.unwrap();
        assert!(
            output
                .response
                .contains("persisted recall survives the retry boundary")
        );
        assert_eq!(
            store
                .events_by_type(session.id, "memory_recall", 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn persisted_memory_recall_rechecks_a_revoked_project_scope() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        store
            .set_session_memory_scope(session.id, "project:tempestmiku")
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, "revoked-memory-retry", "retry the durable turn")
            .await
            .unwrap();
        let memory = MemoryContext::from_records(
            "owner",
            "project:tempestmiku",
            Vec::new(),
            vec![RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: "project:tempestmiku".to_string(),
                text: "revoked project memory must not survive retry".to_string(),
                source: "memory://fixture/revoked-retry".to_string(),
                importance: 0.8,
                created_at: Utc::now(),
            }],
            1_600,
        );
        store
            .append_event_for_turn(
                session.id,
                "memory_recall",
                json!({
                    "schemaVersion": 1,
                    "resourceUri": format!("memory://recalls/{}", turn.id),
                    "context": memory,
                }),
                Some(turn.id),
            )
            .await
            .unwrap();
        store
            .revoke_memory_scope("owner", "project:tempestmiku", "linked folder removed")
            .await
            .unwrap();
        let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "tempestmiku".to_string(),
            path: std::env::current_dir().unwrap(),
            mode: FsMode::Ro,
            commands: Vec::new(),
            safe_args: Vec::new(),
        }])
        .unwrap();
        let state = AppState::new(
            Arc::clone(&store),
            Arc::new(ReplayMustNotQueryMemory),
            Arc::new(EchoChatRunner),
            persona,
            AuthConfig::NoAuth,
        )
        .with_linked_folders(linked)
        .with_auto_turn_dispatcher(false);

        let error = execute_turn_body(&state, &turn).await.unwrap_err();
        assert!(
            matches!(error, ServerError::NotFound(message) if message.contains("project:tempestmiku"))
        );
    }

    #[tokio::test]
    async fn complete_turn_failure_aborts_runtime_before_same_session_retry() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let runner = Arc::new(LinearizedRunner::new(
            Arc::clone(&store),
            OwnershipLossPoint::BeforeComplete,
        ));
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::clone(&runner),
            persona,
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(false);
        let worker_id = Uuid::new_v4();

        store
            .enqueue_turn(session.id, "complete-fails", "first")
            .await
            .unwrap();
        let first = store
            .claim_next_turn(worker_id, Utc::now())
            .await
            .unwrap()
            .unwrap();
        let error = execute_claimed_turn(&state, worker_id, first, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("is not owned"), "{error}");
        assert_eq!(runner.committed.load(Ordering::SeqCst), 0);
        assert_eq!(runner.aborts.load(Ordering::SeqCst), 1);

        store
            .enqueue_turn(session.id, "complete-retry", "second")
            .await
            .unwrap();
        let second = store
            .claim_next_turn(worker_id, Utc::now())
            .await
            .unwrap()
            .unwrap();
        execute_claimed_turn(&state, worker_id, second, Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(*runner.observed.lock().unwrap(), vec![1, 1]);
        assert_eq!(runner.committed.load(Ordering::SeqCst), 1);
        assert_eq!(runner.promotions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn heartbeat_ownership_loss_cancels_and_evicts_before_same_session_retry() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let runner = Arc::new(LinearizedRunner::new(
            Arc::clone(&store),
            OwnershipLossPoint::DuringHeartbeat,
        ));
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::clone(&runner),
            persona,
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(false);
        let worker_id = Uuid::new_v4();

        store
            .enqueue_turn(session.id, "heartbeat-lost", "first")
            .await
            .unwrap();
        let first = store
            .claim_next_turn(worker_id, Utc::now())
            .await
            .unwrap()
            .unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            execute_claimed_turn(&state, worker_id, first, Duration::from_millis(5)),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("is not owned"), "{error}");
        assert!(runner.cancelled.load(Ordering::SeqCst));
        assert_eq!(runner.committed.load(Ordering::SeqCst), 0);
        assert_eq!(runner.aborts.load(Ordering::SeqCst), 1);

        store
            .enqueue_turn(session.id, "heartbeat-retry", "second")
            .await
            .unwrap();
        let second = store
            .claim_next_turn(worker_id, Utc::now())
            .await
            .unwrap()
            .unwrap();
        execute_claimed_turn(&state, worker_id, second, Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(*runner.observed.lock().unwrap(), vec![1, 1]);
        assert_eq!(runner.committed.load(Ordering::SeqCst), 1);
        assert_eq!(runner.promotions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatcher_runs_different_sessions_concurrently() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let runner = Arc::new(ConcurrentRunner::default());
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::clone(&runner),
            persona,
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(false);

        let mut turn_ids = Vec::new();
        for index in 0..3 {
            let session = store
                .create_session(NewSession {
                    mode: assets.modes.default_mode(),
                    persona_status: assets.status.clone(),
                })
                .await
                .unwrap();
            let turn = store
                .enqueue_turn(session.id, &format!("client-{index}"), "hello")
                .await
                .unwrap();
            turn_ids.push(turn.id);
        }

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let dispatcher =
            start_supervised_turn_dispatcher(state, shutdown_rx, 3, Duration::from_millis(5));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let mut all_complete = true;
                for turn_id in &turn_ids {
                    all_complete &= store.turn(*turn_id).await.unwrap().status == "completed";
                }
                if all_complete {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        shutdown_tx.send(true).unwrap();
        dispatcher.await.unwrap();

        assert!(
            runner.max_active.load(Ordering::SeqCst) >= 2,
            "turns from independent sessions should overlap"
        );
    }

    #[tokio::test]
    async fn dispatcher_heartbeats_live_turns_during_periodic_stale_sweeps() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, "heartbeat-live", "take your time")
            .await
            .unwrap();
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::new(SlowRunner),
            persona,
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(false);
        let notify = Arc::clone(&state.turn_notify);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let dispatcher = tokio::spawn(run_turn_dispatcher(
            state,
            notify,
            shutdown_rx,
            1,
            Duration::from_millis(2),
            TurnMaintenance {
                heartbeat_interval: Duration::from_millis(10),
                stale_timeout: Duration::from_millis(80),
                stale_sweep_interval: Duration::from_millis(10),
            },
        ));

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if store.turn(turn.id).await.unwrap().status == "completed" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        shutdown_tx.send(true).unwrap();
        dispatcher.await.unwrap();
        let completed = store.turn(turn.id).await.unwrap();
        assert_eq!(completed.status, "completed");
        assert!(completed.updated_at > completed.started_at.unwrap());
    }

    #[tokio::test]
    async fn dispatcher_periodically_fails_crashed_turns_without_a_restart() {
        let store = Arc::new(InMemoryStore::default());
        store.configure_owner_subject("owner").await.unwrap();
        let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
        let persona = tm_modes::ModesConfig::default();
        let assets = persona.load_assets();
        let session = store
            .create_session(NewSession {
                mode: assets.modes.default_mode(),
                persona_status: assets.status.clone(),
            })
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, "heartbeat-crashed", "lose the worker")
            .await
            .unwrap();
        store
            .claim_next_turn(
                Uuid::new_v4(),
                Utc::now() + chrono::Duration::milliseconds(200),
            )
            .await
            .unwrap()
            .expect("external worker claims turn");
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            Arc::new(ConcurrentRunner::default()),
            persona,
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(false);
        let notify = Arc::clone(&state.turn_notify);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let dispatcher = tokio::spawn(run_turn_dispatcher(
            state,
            notify,
            shutdown_rx,
            1,
            Duration::from_millis(2),
            TurnMaintenance {
                heartbeat_interval: Duration::from_millis(10),
                stale_timeout: Duration::from_millis(40),
                stale_sweep_interval: Duration::from_millis(10),
            },
        ));

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if store.turn(turn.id).await.unwrap().status == "failed" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        shutdown_tx.send(true).unwrap();
        dispatcher.await.unwrap();
        let failed = store.turn(turn.id).await.unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(
            failed.error.as_deref(),
            Some("turn owner heartbeat expired")
        );
    }
}
