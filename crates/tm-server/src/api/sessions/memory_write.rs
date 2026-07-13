use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposeMemoryWriteRequest {
    #[serde(alias = "kind")]
    pub memory_kind: MemoryWriteKind,
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
    Json(payload): Json<ProposeMemoryWriteRequest>,
) -> Result<Json<MemoryWriteProposalResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let session = state.store.get_session(session_id).await?;
    resources::util::validate_authorized_memory_scope(
        &state.linked_folders,
        &session.memory_scope,
    )?;
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
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        crate::evolution::memory_target_class(proposal.memory_kind),
        proposal.proposal_id.to_string(),
        "owner",
        session_id,
        None,
        &proposal,
    );
    let evolution = match evolution {
        Ok(evolution) => evolution,
        Err(error) => {
            let record = crate::evolution::denied_evolution_audit_record(
                crate::evolution::DeniedEvolutionAuditSpec {
                    tier: state.self_evolution_tier,
                    target_class: crate::evolution::memory_target_class(proposal.memory_kind),
                    target_id: proposal.proposal_id.to_string(),
                    actor_id: "owner".to_string(),
                    session_id,
                    dream_id: None,
                    content: &proposal,
                    occurred_at: Utc::now(),
                },
            )?;
            state
                .store
                .append_evolution_audit(crate::EvolutionAuditEntry {
                    idempotency_key: format!("proposal:{}:denied", proposal.proposal_id),
                    record,
                })
                .await?;
            return Err(error);
        }
    };
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

    let (approval_id, approval) = state
        .approval_broker
        .request_permission_detailed_with_effect_for_backend(DurableApprovalSpec {
            session_id,
            origin: "memory".to_string(),
            prompt: memory_write_approval_prompt(&proposal, timeout),
            timeout,
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposal": proposal.clone(),
            }),
            resumable: true,
            sink: Arc::clone(&sink),
        })
        .await?;
    let status = memory_write_status_from_approval(approval.status);
    if let Some(lease) = state
        .store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await?
    {
        let approval = state
            .store
            .approval_request(session_id, approval_id)
            .await?;
        crate::api::approvals::apply_approval_effect_lease(
            state.store.as_ref(),
            &approval,
            &lease,
            state.self_evolution_tier,
            &state.persona,
            Arc::clone(&sink),
        )
        .await?;
    }
    let record = if approval.status == ApprovalStatus::Approved {
        Some(await_persisted_memory_write(state.store.as_ref(), &proposal).await?)
    } else {
        None
    };
    Ok(MemoryWriteProposalResponse {
        proposal_id: proposal.proposal_id,
        memory_kind: proposal.memory_kind,
        status,
        record,
    })
}

pub(super) async fn spawn_personal_assistant_state_capture<S, M, C>(
    state: AppState<S, M, C>,
    session_id: Uuid,
    mode_profile: ModeProfile,
    subject: String,
    scope: String,
    user_content: String,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if !mode_profile.captures_personal_state() {
        return Ok(());
    }
    let proposals = crate::memory::personal_assistant_state_capture_proposals(
        &subject,
        &scope,
        session_id,
        &user_content,
        Utc::now(),
    )?;
    for proposal in proposals {
        enqueue_memory_write_proposal(&state, session_id, proposal, Duration::from_secs(60))
            .await?;
    }
    Ok(())
}

async fn enqueue_memory_write_proposal<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    proposal: MemoryWriteProposal,
    timeout: Duration,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        crate::evolution::memory_target_class(proposal.memory_kind),
        proposal.proposal_id.to_string(),
        "personal-assistant-state-capture",
        session_id,
        None,
        &proposal,
    );
    let evolution = match evolution {
        Ok(evolution) => evolution,
        Err(error) => {
            let record = crate::evolution::denied_evolution_audit_record(
                crate::evolution::DeniedEvolutionAuditSpec {
                    tier: state.self_evolution_tier,
                    target_class: crate::evolution::memory_target_class(proposal.memory_kind),
                    target_id: proposal.proposal_id.to_string(),
                    actor_id: "personal-assistant-state-capture".to_string(),
                    session_id,
                    dream_id: None,
                    content: &proposal,
                    occurred_at: Utc::now(),
                },
            )?;
            state
                .store
                .append_evolution_audit(crate::EvolutionAuditEntry {
                    idempotency_key: format!("proposal:{}:denied", proposal.proposal_id),
                    record,
                })
                .await?;
            return Err(error);
        }
    };
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    sink.emit(
        "write_proposal",
        proposal.event_payload(MemoryWriteStatus::Pending, None),
    )
    .await?;
    state
        .approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "memory".to_string(),
            prompt: memory_write_approval_prompt(&proposal, timeout),
            timeout,
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposal": proposal,
            }),
            resumable: true,
            sink,
        })
        .await?;
    Ok(())
}

fn memory_write_proposal_from_request(
    session_id: Uuid,
    session: &crate::SessionRecord,
    payload: ProposeMemoryWriteRequest,
) -> Result<(MemoryWriteProposal, Duration)> {
    let now = Utc::now();
    let timeout_ms = payload.timeout_ms.unwrap_or(60_000).clamp(1, 60_000);
    let timeout = Duration::from_millis(timeout_ms);
    let subject = session.owner_subject.clone();
    let scope = session.memory_scope.clone();
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
        MemoryWriteKind::ProfileFact => {
            MemoryWriteProposal::profile_fact(crate::memory::ProfileFactProposalInput {
                subject,
                predicate: required_memory_field("predicate", payload.predicate)?,
                object: required_memory_field("object", payload.object)?,
                confidence: payload.confidence.unwrap_or(0.8),
                source,
                provenance_label,
                provenance,
                created_at: now,
            })?
        }
        MemoryWriteKind::RecallChunk => MemoryWriteProposal::recall_chunk(
            subject,
            scope,
            required_memory_field("text", payload.text)?,
            source,
            provenance_label,
            provenance,
            now,
        )?,
    };
    ensure_memory_proposal_contains_no_sensitive_data(&proposal)?;
    Ok((proposal, timeout))
}

fn ensure_memory_proposal_contains_no_sensitive_data(proposal: &MemoryWriteProposal) -> Result<()> {
    let provenance = serde_json::to_string(&proposal.provenance)
        .map_err(|err| ServerError::InvalidRequest(format!("invalid memory provenance: {err}")))?;
    for value in [
        proposal.subject.as_str(),
        proposal.scope.as_str(),
        proposal.text.as_str(),
        proposal.source.as_str(),
        proposal.provenance_label.as_str(),
        provenance.as_str(),
    ] {
        if tm_memory::contains_sensitive_data(value) {
            return Err(ServerError::InvalidRequest(
                "memory proposal contains sensitive data".to_string(),
            ));
        }
    }
    Ok(())
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
            proposal.preview_text()
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

async fn await_persisted_memory_write<S>(
    store: &S,
    proposal: &MemoryWriteProposal,
) -> Result<MemoryRecordRef>
where
    S: Store,
{
    for _ in 0..100 {
        let result = match proposal.memory_kind {
            MemoryWriteKind::ProfileFact => store
                .profile_fact(&proposal.subject, proposal.record_id)
                .await
                .map(|_| ()),
            MemoryWriteKind::RecallChunk => store
                .recall_chunk(&proposal.scope, proposal.record_id)
                .await
                .map(|_| ()),
        };
        match result {
            Ok(()) => return Ok(proposal.record_ref()),
            Err(ServerError::NotFound(_)) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ServerError::Store(format!(
        "approved memory effect {} was not applied",
        proposal.proposal_id
    )))
}

fn memory_write_status_from_approval(status: ApprovalStatus) -> MemoryWriteStatus {
    match status {
        ApprovalStatus::Approved => MemoryWriteStatus::Approved,
        ApprovalStatus::Denied => MemoryWriteStatus::Denied,
        ApprovalStatus::TimedOut => MemoryWriteStatus::TimedOut,
        ApprovalStatus::Cancelled => MemoryWriteStatus::Cancelled,
    }
}
