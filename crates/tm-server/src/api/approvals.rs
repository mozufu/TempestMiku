use super::*;

pub(super) async fn resolve_approval<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, approval_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<ResolveApprovalRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let approval = state
        .store
        .approval_request(session_id, approval_id)
        .await?;
    let sink: Arc<dyn CodingEventSink> = match approval.turn_id {
        Some(turn_id) => Arc::new(StoreCodingEventSink::for_turn(
            session_id,
            turn_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        )),
        None => Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        )),
    };
    state
        .approval_broker
        .resolve_persisted(session_id, approval_id, payload, sink)
        .await?;
    apply_approval_effect(&state, session_id, approval_id).await?;
    Ok(Json(json!({ "status": "resolved" })))
}

async fn apply_approval_effect<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    approval_id: Uuid,
) -> Result<()>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let owner_id = Uuid::new_v4();
    let Some(lease) = state
        .store
        .claim_approval_effect(
            approval_id,
            owner_id,
            Utc::now(),
            chrono::Duration::seconds(30),
        )
        .await?
    else {
        return Ok(());
    };
    let approval = state
        .store
        .approval_request(session_id, approval_id)
        .await?;
    let sink: Arc<dyn CodingEventSink> = match approval.turn_id {
        Some(turn_id) => Arc::new(StoreCodingEventSink::for_turn(
            session_id,
            turn_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        )),
        None => Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        )),
    };
    apply_approval_effect_lease(
        state.store.as_ref(),
        &approval,
        &lease,
        state.self_evolution_tier,
        &state.persona,
        sink,
    )
    .await
}

pub(crate) async fn apply_approval_effect_lease<S>(
    store: &S,
    approval: &ApprovalRequestRecord,
    lease: &crate::ApprovalEffectLease,
    self_evolution_tier: SelfEvolutionTier,
    persona: &tm_modes::ModesConfig,
    sink: Arc<dyn CodingEventSink>,
) -> Result<()>
where
    S: Store,
{
    if matches!(
        lease.effect.effect_type.as_str(),
        "approval_continuation" | "approval_resolution"
    ) {
        store.complete_approval_effect(lease, Utc::now()).await?;
        return Ok(());
    }

    store
        .heartbeat_approval_effect(lease, Utc::now())
        .await
        .map_err(|error| match error {
            ServerError::NotFound(_) | ServerError::Conflict(_) => crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::StaleApproval,
                format!(
                    "approval effect {} owner {} epoch {} is stale",
                    lease.effect.id, lease.owner_id, lease.epoch
                ),
            ),
            other => other,
        })?;

    let proposal_payload = match lease.effect.effect_type.as_str() {
        "memory_write" => {
            apply_memory_write_effect(store, approval, &lease.effect, self_evolution_tier).await
        }
        "skill_write" => {
            apply_skill_write_effect(store, approval, &lease.effect, self_evolution_tier).await
        }
        "evolution_review" => {
            apply_evolution_review_effect(
                store,
                approval,
                &lease.effect,
                self_evolution_tier,
                persona,
            )
            .await
        }
        other => Err(ServerError::Store(format!(
            "unknown approval effect type {other}"
        ))),
    };
    let proposal_payload = match proposal_payload {
        Ok(payload) => payload,
        Err(error) => return fail_approval_effect(store, lease, error).await,
    };
    let (_, event) = match store
        .complete_approval_effect_with_event(lease, proposal_payload, sink.turn_id(), Utc::now())
        .await
    {
        Ok(finalized) => finalized,
        Err(error) => return fail_approval_effect(store, lease, error).await,
    };

    // The transaction above is the source of truth. A process crash or local broadcast failure
    // after this point leaves one replayable event and an applied effect; it must not requeue the
    // already-finalized mutation.
    sink.publish_persisted(event).await
}

async fn fail_approval_effect<S>(
    store: &S,
    lease: &crate::ApprovalEffectLease,
    error: ServerError,
) -> Result<()>
where
    S: Store,
{
    store
        .fail_approval_effect(
            lease,
            &error.to_string(),
            Utc::now() + chrono::Duration::seconds(5),
            3,
        )
        .await?;
    Err(error)
}

async fn apply_memory_write_effect<S>(
    store: &S,
    approval: &ApprovalRequestRecord,
    effect: &ApprovalEffectRecord,
    self_evolution_tier: SelfEvolutionTier,
) -> Result<Value>
where
    S: Store,
{
    let proposal: MemoryWriteProposal = serde_json::from_value(
        effect
            .payload_json
            .get("proposal")
            .cloned()
            .ok_or_else(|| ServerError::Store("memory effect missing proposal".to_string()))?,
    )?;
    let status = approval_memory_status(&approval.status)?;
    let record = if approval.status == "approved" {
        let digest = crate::evolution::evolution_content_digest(&proposal)?;
        crate::evolution::verify_approved_evolution_effect(
            self_evolution_tier,
            &effect.payload_json,
            crate::evolution::memory_target_class(proposal.memory_kind),
            &proposal.proposal_id.to_string(),
            &digest,
        )?;
        match proposal.memory_kind {
            MemoryWriteKind::ProfileFact => {
                store
                    .upsert_profile_fact(crate::memory::profile_fact_record(&proposal)?)
                    .await?;
            }
            MemoryWriteKind::RecallChunk => {
                store
                    .upsert_recall_chunk(crate::memory::recall_chunk_record(&proposal)?)
                    .await?;
            }
        }
        Some(proposal.record_ref())
    } else {
        None
    };
    Ok(proposal.event_payload(status, record.as_ref()))
}

async fn apply_skill_write_effect<S>(
    store: &S,
    approval: &ApprovalRequestRecord,
    effect: &ApprovalEffectRecord,
    self_evolution_tier: SelfEvolutionTier,
) -> Result<Value>
where
    S: Store,
{
    let proposal_id = effect
        .payload_json
        .get("proposalId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ServerError::Store("skill effect missing proposalId".to_string()))?;
    let status = approval_skill_status(&approval.status)?;
    let proposal = store.skill_proposal(proposal_id).await?;
    if approval.status == "approved" {
        let digest = crate::evolution::evolution_content_digest(&proposal)?;
        crate::evolution::verify_approved_evolution_effect(
            self_evolution_tier,
            &effect.payload_json,
            tm_host::EvolutionTargetClass::SkillProposal,
            &proposal_id.to_string(),
            &digest,
        )?;
    }
    let proposal = store
        .update_skill_proposal_status(proposal_id, status)
        .await?;
    Ok(crate::dream::proposals::skill_proposal_payload(
        &proposal, status,
    ))
}

async fn apply_evolution_review_effect<S>(
    store: &S,
    approval: &ApprovalRequestRecord,
    effect: &ApprovalEffectRecord,
    self_evolution_tier: SelfEvolutionTier,
    persona: &tm_modes::ModesConfig,
) -> Result<Value>
where
    S: Store,
{
    let proposal_id = effect
        .payload_json
        .get("proposalId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ServerError::Store("review effect missing proposalId".to_string()))?;
    let status = approval_review_status(&approval.status)?;
    let proposal = store.evolution_review_proposal(proposal_id).await?;
    if approval.status == "approved" {
        let digest = tm_host::EvolutionDigest::new(proposal.content_digest.clone())
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        crate::evolution::verify_approved_review_effect(
            self_evolution_tier,
            &effect.payload_json,
            crate::api::sessions::evolution_review::review_target_class(&proposal.target),
            &proposal_id.to_string(),
            &digest,
        )?;
        let current = crate::api::sessions::evolution_review::review_base_snapshot(
            persona,
            &proposal.target,
        )?;
        if current != (proposal.base_version, proposal.base_digest.clone()) {
            return Err(crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::StaleApproval,
                format!("review proposal {proposal_id} base changed after approval"),
            ));
        }
        let current_digest = crate::api::sessions::evolution_review::review_content_digest(
            &proposal.target,
            proposal.base_version,
            &proposal.base_digest,
            &proposal.changes,
        )?;
        if current_digest != proposal.content_digest {
            return Err(crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::InvalidPayload,
                format!("review proposal {proposal_id} digest changed"),
            ));
        }
    }
    let proposal = store
        .update_evolution_review_proposal_status(proposal_id, status)
        .await?;
    Ok(crate::api::sessions::evolution_review::evolution_review_proposal_payload(&proposal))
}

fn approval_memory_status(status: &str) -> Result<MemoryWriteStatus> {
    match status {
        "approved" => Ok(MemoryWriteStatus::Approved),
        "denied" => Ok(MemoryWriteStatus::Denied),
        "timed_out" => Ok(MemoryWriteStatus::TimedOut),
        "cancelled" => Ok(MemoryWriteStatus::Cancelled),
        other => Err(ServerError::Store(format!(
            "approval has non-terminal status {other}"
        ))),
    }
}

fn approval_review_status(status: &str) -> Result<tm_modes::ReviewProposalStatus> {
    use tm_modes::ReviewProposalStatus::{Approved, Cancelled, Denied, TimedOut};
    match status {
        "approved" => Ok(Approved),
        "denied" => Ok(Denied),
        "timed_out" => Ok(TimedOut),
        "cancelled" => Ok(Cancelled),
        other => Err(ServerError::Store(format!(
            "approval has non-terminal status {other}"
        ))),
    }
}

fn approval_skill_status(status: &str) -> Result<SkillProposalStatus> {
    match status {
        "approved" => Ok(SkillProposalStatus::Approved),
        "denied" => Ok(SkillProposalStatus::Denied),
        "timed_out" => Ok(SkillProposalStatus::TimedOut),
        "cancelled" => Ok(SkillProposalStatus::Cancelled),
        other => Err(ServerError::Store(format!(
            "approval has non-terminal status {other}"
        ))),
    }
}
