use super::super::*;
use super::{
    dispatcher::{ExecutedTurn, TurnRuntime},
    drive_recall::persist_drive_recall_chunks,
};
use tm_core::Message;
use tm_host::{HostFn, ResourceHandler};

pub(super) async fn execute_turn_body<S, M, C>(
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
    resources::util::validate_authorized_project(
        state.store.as_ref(),
        session.project_id.as_deref(),
    )
    .await?;
    let prior_messages =
        bounded_prior_messages(state.store.as_ref(), session_id, durable_turn.id).await?;
    // Long-term memory is pull-only. Modes may grant memory.search, but retrieval happens only if
    // the model calls it through execute. Mode changes remain separately gated.
    let turn_profile = mode_profile(&state.persona, &session.mode_state.mode);
    let project_id = session.project_id.clone();
    let memory_scope = session.memory_scope();
    let subject = session.owner_subject.clone();
    let persona_prompt = build_turn_prompt(state, &session.mode_state.mode, &durable_turn.content);
    state
        .store
        .ensure_memory_scope_active(&subject, &memory_scope)
        .await?;
    let user_prompt = durable_turn.content.clone();
    let memory_search_host: Arc<dyn HostFn> = Arc::new(MemorySearchHostFn::new(state));
    let mode_suggest_host: Arc<dyn HostFn> =
        Arc::new(ModeSuggestHostFn::new(state, MODE_SUGGEST_APPROVAL_TIMEOUT));
    let mut chat_capabilities = turn_profile.capabilities.clone();
    state.extend_egress_turn_capabilities(&mut chat_capabilities);
    state.extend_mcp_turn_capabilities(&mut chat_capabilities);
    if session.mode_state.lock_source.is_none() && !turn_profile.has_capability("backend.coding") {
        chat_capabilities.push(MODE_SUGGEST_CAPABILITY.to_string());
    }
    chat_capabilities.sort();
    chat_capabilities.dedup();
    // Handler registration is independent from authority. Cached chat sessions keep both stable
    // services, while the exact per-turn capability set remains the enforcement boundary.
    let chat_host_functions = vec![memory_search_host, mode_suggest_host];
    let project_resource_handlers = project_id
        .as_ref()
        .map(|project_id| {
            vec![Arc::new(crate::ProjectEnvironmentResourceHandler::new(
                Arc::clone(&state.store),
                subject.clone(),
                project_id.clone(),
            )) as Arc<dyn ResourceHandler>]
        })
        .unwrap_or_default();

    let (response, runtime) = if turn_profile.has_capability("backend.coding") {
        let backend = state.coding_backend.as_ref().ok_or_else(|| {
            ServerError::Backend(
                "backend.coding is granted but no coding backend is configured".to_string(),
            )
        })?;
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
                    owner_subject: subject.clone(),
                    project_id: project_id.clone(),
                    memory_scope: memory_scope.clone(),
                    capabilities: chat_capabilities.clone(),
                    prior_messages: prior_messages.clone(),
                    resource_handlers: project_resource_handlers.clone(),
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
                    system_prompt: persona_prompt.system_prompt,
                    mode: session.mode_state.mode.clone(),
                    owner_subject: subject.clone(),
                    project_id: project_id.clone(),
                    memory_scope: memory_scope.clone(),
                    capabilities: chat_capabilities,
                    prior_messages,
                    dialectic: None,
                    limits: crate::ChatRunLimits::default(),
                    deny_approvals: false,
                    host_functions: chat_host_functions,
                    resource_handlers: project_resource_handlers,
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

    persist_drive_recall_chunks(state, &memory_scope).await?;
    if turn_profile.has_capability("backend.coding") && !response.trim().is_empty() {
        state
            .store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: memory_scope.clone(),
                text: format!(
                    "Project summary/open loop from session {session_id}: {}",
                    response.trim()
                ),
                source: format!("session:{session_id}:assistant"),
                importance: 0.58,
                created_at: Utc::now(),
            })
            .await?;
        if let Some(project_id) = project_id {
            record_project_observations(
                state,
                session_id,
                project_id,
                &durable_turn.content,
                &response,
            )
            .await?;
        }
    }
    let auto_persona_candidate = if session.mode_state.lock_source.is_none()
        && !turn_profile.has_capability("backend.coding")
        && state.self_evolution_tier == tm_host::SelfEvolutionTier::Moderate
        && state.persona.managed_persona_addenda_path().is_some()
        && let Some(event) = state
            .store
            .event_for_turn(session_id, durable_turn.id, "memory_recall")
            .await?
    {
        let memory = persisted_memory_context(&event, &subject, &memory_scope)?;
        super::super::persona_candidate::detect(durable_turn.id, &durable_turn.content, &memory)
    } else {
        None
    };
    super::super::memory_write::spawn_personal_assistant_state_capture(
        state.clone(),
        session_id,
        turn_profile,
        subject,
        memory_scope,
        durable_turn.content.clone(),
    )
    .await?;
    Ok(ExecutedTurn {
        response,
        runtime,
        auto_persona_candidate,
    })
}

fn persisted_memory_context(
    event: &crate::SessionEvent,
    subject: &str,
    scope: &str,
) -> Result<crate::MemoryContext> {
    let memory: crate::MemoryContext =
        serde_json::from_value(event.payload_json.get("context").cloned().ok_or_else(|| {
            ServerError::Store("persisted memory search is missing its context".to_string())
        })?)
        .map_err(|error| {
            ServerError::Store(format!("persisted memory search is invalid: {error}"))
        })?;
    if memory.subject != subject || memory.scope != scope {
        return Err(ServerError::Store(
            "persisted memory search authority does not match the durable turn".to_string(),
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
