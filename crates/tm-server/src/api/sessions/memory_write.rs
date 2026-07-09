use super::*;

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

pub(crate) async fn propose_memory_write<S, M, C>(
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
    let (proposal, timeout) =
        memory_write_proposal_from_request(&state.persona, session_id, &session, payload)?;
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

pub(super) fn spawn_personal_assistant_state_capture<S, M, C>(
    state: AppState<S, M, C>,
    session_id: Uuid,
    mode_profile: ModeProfile,
    subject: String,
    scope: String,
    user_content: String,
) where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if !mode_profile.captures_personal_state() {
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
    persona: &ModesConfig,
    session_id: Uuid,
    session: &crate::SessionRecord,
    payload: ProposeMemoryWriteRequest,
) -> Result<(MemoryWriteProposal, Duration)> {
    let now = Utc::now();
    let timeout_ms = payload.timeout_ms.unwrap_or(60_000).clamp(1, 60_000);
    let timeout = Duration::from_millis(timeout_ms);
    let scope = payload
        .scope
        .unwrap_or_else(|| mode_profile(persona, &session.mode_state.mode).default_scope);
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
