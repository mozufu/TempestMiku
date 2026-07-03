use super::*;

#[derive(Debug, Default, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub mode: Mode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub id: Uuid,
    pub mode: Mode,
    pub mode_state: ModeState,
    pub persona_status: crate::PersonaStatus,
    pub label: String,
    pub voice_cap: String,
    pub default_scope: String,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
}

pub(super) async fn create_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
    payload: Option<Json<CreateSessionRequest>>,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let mode = payload
        .map(|Json(payload)| payload.mode)
        .unwrap_or_default();
    let persona_status = state.persona.load_status();
    let session = state
        .store
        .create_session(NewSession {
            mode,
            persona_status: persona_status.clone(),
        })
        .await?;
    let mode_event = state
        .store
        .append_event(
            session.id,
            "mode",
            mode_changed_payload(None, &session.mode_state, persona_status.clone())?,
        )
        .await?;
    let _ = state.sender(session.id).send(mode_event);
    Ok(Json(session_response(session, persona_status)))
}

pub(super) async fn get_session<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<CreateSessionResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let persona_status = session.persona_status.clone();
    Ok(Json(session_response(session, persona_status)))
}

fn session_response(
    session: crate::SessionRecord,
    persona_status: crate::PersonaStatus,
) -> CreateSessionResponse {
    CreateSessionResponse {
        id: session.id,
        mode: session.mode,
        mode_state: session.mode_state.clone(),
        persona_status,
        label: session.mode.label().to_string(),
        voice_cap: session.mode.voice_cap().to_string(),
        default_scope: session.mode.default_scope().to_string(),
        active_skills: active_skills(session.mode),
    }
}

#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub content: String,
    #[serde(default = "default_subject")]
    pub subject: String,
    #[serde(default)]
    pub scope: Option<String>,
}

pub(super) fn default_subject() -> String {
    "brian".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposeMemoryWriteRequest {
    #[serde(alias = "kind")]
    pub memory_kind: MemoryWriteKind,
    #[serde(default = "default_subject")]
    pub subject: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub predicate: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub provenance_label: Option<String>,
    #[serde(default)]
    pub provenance: Option<Value>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWriteProposalResponse {
    pub proposal_id: Uuid,
    pub memory_kind: MemoryWriteKind,
    pub status: MemoryWriteStatus,
    pub record: Option<MemoryRecordRef>,
}

pub(super) async fn post_message<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PostMessageRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let mut session = state.store.get_session(session_id).await?;
    let (suggested, reason) = route_mode_for_prompt(&payload.content);
    let preserves_serious_override = session.mode_state.mode == Mode::SeriousEngineer
        && session.mode_state.override_source.is_some()
        && suggested == Mode::PersonalAssistant;
    if session.mode_state.lock_source.is_none()
        && suggested != session.mode_state.mode
        && !preserves_serious_override
    {
        let mut next = session.mode_state.clone();
        next.mode = suggested;
        next.router_reason = Some(reason);
        next.override_source = None;
        next.updated_at = Utc::now();
        let (updated, _) = commit_mode_state(&state, session.clone(), next).await?;
        session = updated;
    }
    let scope = payload
        .scope
        .clone()
        .unwrap_or_else(|| session.mode_state.mode.default_scope().to_string());
    let persona_prompt = build_turn_prompt(&state, session.mode_state.mode);
    state
        .store
        .append_message(session_id, "user", &payload.content)
        .await?;
    let memory = state
        .memory
        .context_for_turn(&payload.subject, &scope, &payload.content)
        .await?;
    let user_prompt = if memory.is_empty() {
        payload.content.clone()
    } else {
        format!(
            "{}\n\n[recall]\n{}",
            payload.content,
            memory.render_prompt_block()
        )
    };
    let response = if matches!(
        session.mode_state.mode,
        Mode::SeriousEngineer | Mode::Handoff
    ) {
        if let Some(backend) = &state.coding_backend {
            let sink = Arc::new(StoreCodingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            backend
                .run_turn(
                    CodingTurn {
                        session_id,
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode,
                        scope: scope.clone(),
                    },
                    sink,
                )
                .await?
                .final_text
        } else {
            let sink = Arc::new(PersistingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            let response = state
                .chat
                .run_turn(
                    ChatTurn {
                        session_id,
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode,
                        scope: scope.clone(),
                    },
                    sink.clone(),
                )
                .await?;
            sink.flush().await?;
            response
        }
    } else {
        let sink = Arc::new(PersistingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        let response = state
            .chat
            .run_turn(
                ChatTurn {
                    session_id,
                    user_prompt,
                    system_prompt: persona_prompt.system_prompt,
                    mode: session.mode_state.mode,
                    scope: scope.clone(),
                },
                sink.clone(),
            )
            .await?;
        sink.flush().await?;
        response
    };

    state
        .store
        .append_message(session_id, "assistant", &response)
        .await?;
    if matches!(
        session.mode_state.mode,
        Mode::SeriousEngineer | Mode::Handoff
    ) && !response.trim().is_empty()
    {
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
                created_at: Utc::now(),
            })
            .await?;
        record_project_observations(
            &state,
            session_id,
            project_id_from_scope(&scope),
            &payload.content,
            &response,
        )
        .await?;
    }
    spawn_personal_assistant_state_capture(
        state.clone(),
        session_id,
        session.mode_state.mode,
        payload.subject,
        scope,
        payload.content,
    );
    Ok(Json(json!({ "status": "accepted" })))
}

pub(super) async fn propose_memory_write<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ProposeMemoryWriteRequest>,
) -> Result<Json<MemoryWriteProposalResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    let (proposal, timeout) = memory_write_proposal_from_request(session_id, &session, payload)?;
    Ok(Json(
        run_memory_write_proposal(&state, session_id, proposal, timeout).await?,
    ))
}

async fn run_memory_write_proposal<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    proposal: MemoryWriteProposal,
    timeout: Duration,
) -> Result<MemoryWriteProposalResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let sink: Arc<dyn crate::CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    sink.emit(
        "write_proposal",
        proposal.event_payload(MemoryWriteStatus::Pending, None),
    )
    .await?;

    let approval = state
        .approval_broker
        .request_permission_detailed_for_backend(
            session_id,
            "memory",
            memory_write_approval_prompt(&proposal, timeout),
            timeout,
            Arc::clone(&sink),
        )
        .await?;
    let status = memory_write_status_from_approval(approval.status);
    let record = if approval.status == ApprovalStatus::Approved {
        Some(persist_memory_write(state.store.as_ref(), &proposal).await?)
    } else {
        None
    };
    sink.emit(
        "write_proposal",
        proposal.event_payload(status, record.as_ref()),
    )
    .await?;
    Ok(MemoryWriteProposalResponse {
        proposal_id: proposal.proposal_id,
        memory_kind: proposal.memory_kind,
        status,
        record,
    })
}

fn spawn_personal_assistant_state_capture<S, M, C>(
    state: AppState<S, M, C>,
    session_id: Uuid,
    mode: Mode,
    subject: String,
    scope: String,
    user_content: String,
) where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if mode != Mode::PersonalAssistant {
        return;
    }
    let proposals = match crate::memory::personal_assistant_state_capture_proposals(
        &subject,
        &scope,
        session_id,
        &user_content,
        Utc::now(),
    ) {
        Ok(proposals) => proposals,
        Err(err) => {
            tracing::warn!(%err, %session_id, "state capture proposal extraction failed");
            return;
        }
    };
    for proposal in proposals {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) =
                run_memory_write_proposal(&state, session_id, proposal, Duration::from_secs(60))
                    .await
            {
                tracing::warn!(%err, %session_id, "state capture memory proposal failed");
            }
        });
    }
}

fn memory_write_proposal_from_request(
    session_id: Uuid,
    session: &crate::SessionRecord,
    payload: ProposeMemoryWriteRequest,
) -> Result<(MemoryWriteProposal, Duration)> {
    let now = Utc::now();
    let timeout_ms = payload.timeout_ms.unwrap_or(60_000).clamp(1, 60_000);
    let timeout = Duration::from_millis(timeout_ms);
    let scope = payload
        .scope
        .unwrap_or_else(|| session.mode_state.mode.default_scope().to_string());
    let source = payload
        .source
        .unwrap_or_else(|| format!("session:{session_id}:memory-write"));
    let provenance_label = payload
        .provenance_label
        .unwrap_or_else(|| "manual-memory-proposal".to_string());
    let provenance = payload.provenance.unwrap_or_else(|| {
        json!({
            "label": provenance_label,
            "source": source,
            "sourceSession": session_id,
            "mode": session.mode_state.mode,
            "proposedAt": now,
        })
    });
    let proposal = match payload.memory_kind {
        MemoryWriteKind::ProfileFact => MemoryWriteProposal::profile_fact(
            payload.subject,
            required_memory_field("predicate", payload.predicate)?,
            required_memory_field("object", payload.object)?,
            payload.confidence.unwrap_or(0.8),
            source,
            provenance_label,
            provenance,
            now,
        )?,
        MemoryWriteKind::RecallChunk => MemoryWriteProposal::recall_chunk(
            payload.subject,
            scope,
            required_memory_field("text", payload.text)?,
            source,
            provenance_label,
            provenance,
            now,
        )?,
    };
    Ok((proposal, timeout))
}

fn required_memory_field(field: &str, value: Option<String>) -> Result<String> {
    value.ok_or_else(|| ServerError::InvalidRequest(format!("memory {field} is required")))
}

fn memory_write_approval_prompt(
    proposal: &MemoryWriteProposal,
    timeout: Duration,
) -> crate::ApprovalPrompt {
    crate::ApprovalPrompt {
        action: format!(
            "memory.write {}: {}",
            proposal.memory_kind.as_str(),
            proposal.text
        ),
        scope: json!({
            "proposal": proposal.approval_scope(),
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            crate::ApprovalOption {
                option_id: "allow".to_string(),
                name: "Save memory".to_string(),
                kind: "allow_once".to_string(),
            },
            crate::ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject memory".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

async fn persist_memory_write<S>(
    store: &S,
    proposal: &MemoryWriteProposal,
) -> Result<MemoryRecordRef>
where
    S: Store,
{
    match proposal.memory_kind {
        MemoryWriteKind::ProfileFact => {
            let fact = crate::memory::profile_fact_record(proposal)?;
            store.upsert_profile_fact(fact).await?;
        }
        MemoryWriteKind::RecallChunk => {
            let chunk = crate::memory::recall_chunk_record(proposal)?;
            store.upsert_recall_chunk(chunk).await?;
        }
    }
    Ok(proposal.record_ref())
}

fn memory_write_status_from_approval(status: ApprovalStatus) -> MemoryWriteStatus {
    match status {
        ApprovalStatus::Approved => MemoryWriteStatus::Approved,
        ApprovalStatus::Denied => MemoryWriteStatus::Denied,
        ApprovalStatus::TimedOut => MemoryWriteStatus::TimedOut,
        ApprovalStatus::Cancelled => MemoryWriteStatus::Cancelled,
    }
}
