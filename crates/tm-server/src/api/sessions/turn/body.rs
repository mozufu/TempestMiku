use super::super::*;
use super::{
    dispatcher::{ExecutedTurn, TurnRuntime},
    drive_recall::{drive_recall_block, persist_drive_recall_chunks},
};
use tm_core::Message;
use tm_host::HostFn;

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
    resources::util::validate_authorized_memory_scope(state.store.as_ref(), &session.memory_scope)
        .await?;
    let (prior_messages, turn_number) =
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
    let dialectic = dialectic_for_turn(
        state.store.as_ref(),
        session_id,
        durable_turn.id,
        turn_number,
        &turn_profile,
        &durable_turn.content,
        &memory,
    )
    .await?;
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
    state.extend_egress_turn_capabilities(&mut chat_capabilities);
    state.extend_mcp_turn_capabilities(&mut chat_capabilities);
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
                        capabilities: chat_capabilities.clone(),
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
                        dialectic: dialectic.clone(),
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
                    dialectic,
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
    let auto_persona_candidate = if session.mode_state.lock_source.is_none()
        && !turn_profile.has_capability("backend.coding")
        && state.self_evolution_tier == tm_host::SelfEvolutionTier::Moderate
        && state.persona.managed_persona_addenda_path().is_some()
    {
        super::super::persona_candidate::detect(durable_turn.id, &durable_turn.content, &memory)
    } else {
        None
    };
    super::super::memory_write::spawn_personal_assistant_state_capture(
        state.clone(),
        session_id,
        turn_profile,
        subject,
        scope,
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
) -> Result<(Vec<Message>, u64)> {
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
    let turn_number = stored
        .iter()
        .filter(|message| message.seq <= current_seq && message.role == "user")
        .count()
        .try_into()
        .map_err(|_| ServerError::Store("session turn count exceeds u64".to_string()))?;
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
    Ok((newest, turn_number))
}

async fn dialectic_for_turn<S: Store>(
    store: &S,
    session_id: Uuid,
    turn_id: Uuid,
    turn_number: u64,
    profile: &ModeProfile,
    query: &str,
    memory: &crate::MemoryContext,
) -> Result<Option<crate::DialecticTurn>> {
    let serious_or_engineering =
        profile.capability_class == "engineering" || profile.has_capability("backend.coding");
    let facts = memory
        .profile_facts
        .iter()
        .map(|fact| tm_memory::DialecticFact::new(fact.text.clone(), fact.source_uri.clone()));
    let Some(request) = tm_memory::DialecticRequest::plan(
        turn_number,
        serious_or_engineering,
        &memory.subject,
        query,
        facts,
    ) else {
        return Ok(None);
    };

    let Some(event) = store
        .event_for_turn(session_id, turn_id, tm_memory::DIALECTIC_EVENT_TYPE)
        .await?
    else {
        return Ok(Some(crate::DialecticTurn::Generate(request)));
    };
    let trace: tm_memory::DialecticTrace =
        serde_json::from_value(event.payload_json).map_err(|error| {
            ServerError::Store(format!("persisted dialectic trace is invalid: {error}"))
        })?;
    trace
        .validate_for(&request)
        .map_err(|error| ServerError::Store(error.to_string()))?;
    if trace.status == tm_memory::DialecticStatus::Applied {
        Ok(Some(crate::DialecticTurn::Reuse(trace)))
    } else {
        // An unavailable or empty auxiliary pass is durable for this turn. Do not keep retrying
        // an optional model role while the main response can still proceed without it.
        Ok(None)
    }
}
