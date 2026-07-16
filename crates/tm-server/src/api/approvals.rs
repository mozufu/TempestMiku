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
            apply_skill_write_effect(store, approval, &lease.effect, self_evolution_tier, persona)
                .await
        }
        "skill_rollback" => {
            apply_skill_rollback_effect(approval, &lease.effect, self_evolution_tier, persona).await
        }
        "mode_addendum_rollback" => {
            apply_mode_addendum_rollback_effect(
                approval,
                &lease.effect,
                self_evolution_tier,
                persona,
            )
            .await
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

async fn apply_mode_addendum_rollback_effect(
    approval: &ApprovalRequestRecord,
    effect: &ApprovalEffectRecord,
    self_evolution_tier: SelfEvolutionTier,
    persona: &tm_modes::ModesConfig,
) -> Result<Value> {
    let rollback = effect
        .payload_json
        .get("rollback")
        .cloned()
        .ok_or_else(|| {
            ServerError::Store("mode addendum rollback effect missing rollback".to_string())
        })?;
    let mode_id = rollback
        .get("modeId")
        .and_then(Value::as_str)
        .map(tm_modes::ModeId::new)
        .ok_or_else(|| {
            ServerError::Store("mode addendum rollback effect missing modeId".to_string())
        })?;
    let expected_active_digest = rollback
        .get("expectedActiveDigest")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ServerError::Store(
                "mode addendum rollback effect missing expectedActiveDigest".to_string(),
            )
        })?;
    let target_digest = rollback.get("targetDigest").and_then(Value::as_str);
    let evolution_target_id = rollback
        .get("evolutionTargetId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ServerError::Store(
                "mode addendum rollback effect missing evolutionTargetId".to_string(),
            )
        })?;
    let status = approval_terminal_status(&approval.status)?;
    let activation = if approval.status == "approved" {
        let digest = crate::evolution::evolution_content_digest(&rollback)?;
        crate::evolution::verify_approved_review_effect(
            self_evolution_tier,
            &effect.payload_json,
            tm_host::EvolutionTargetClass::ModeProposal,
            evolution_target_id,
            &digest,
        )?;
        let managed = persona
            .managed_mode_addendum(&mode_id)
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        let active = managed.active.ok_or_else(|| {
            crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::StaleApproval,
                format!("managed mode addendum {mode_id} is no longer active"),
            )
        })?;
        if active.content_digest != expected_active_digest {
            return Err(crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::StaleApproval,
                format!(
                    "managed mode addendum {mode_id} active version changed from {expected_active_digest} to {}",
                    active.content_digest
                ),
            ));
        }
        if let Some(target_digest) = target_digest
            && !managed
                .versions
                .iter()
                .any(|version| version.content_digest == target_digest)
        {
            return Err(ServerError::NotFound(format!(
                "managed mode addendum {mode_id} version {target_digest}"
            )));
        }
        Some(
            persona
                .rollback_managed_mode_addendum(&mode_id, expected_active_digest, target_digest)
                .map_err(|error| {
                    crate::evolution::policy_error(
                        tm_host::EvolutionPolicyReason::StaleApproval,
                        error.to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    Ok(
        crate::api::sessions::evolution_review::mode_addendum_rollback_payload(
            &rollback,
            status,
            activation.as_ref(),
        ),
    )
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
        Some(store.apply_approved_memory_proposal(&proposal).await?)
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
        .ok_or_else(|| ServerError::Store("skill effect missing proposalId".to_string()))?;
    let status = approval_skill_status(&approval.status)?;
    let proposal = store.skill_proposal(proposal_id).await?;
    let installation = if approval.status == "approved" {
        let digest = crate::evolution::evolution_content_digest(&proposal)?;
        crate::evolution::verify_approved_evolution_effect(
            self_evolution_tier,
            &effect.payload_json,
            tm_host::EvolutionTargetClass::SkillProposal,
            &proposal_id.to_string(),
            &digest,
        )?;
        let lifecycle = tm_memory::skill_proposal_lifecycle(&proposal);
        if !lifecycle.installable {
            return Err(crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::InvalidPayload,
                format!(
                    "skill proposal {proposal_id} is not installable: {}",
                    lifecycle.violations.join(", ")
                ),
            ));
        }
        let activation = persona
            .install_managed_skill(tm_modes::ManagedSkillInstall {
                name: lifecycle.normalized_name,
                body: proposal.body.clone(),
                content_digest: lifecycle.content_digest,
                source_proposal_id: proposal.id.to_string(),
                description: proposal.description.clone(),
                triggers: vec![proposal.trigger.clone()],
                use_criteria: proposal.use_criteria.clone(),
            })
            .map_err(|error| {
                crate::evolution::policy_error(
                    tm_host::EvolutionPolicyReason::InvalidPayload,
                    error.to_string(),
                )
            })?;
        Some(activation)
    } else {
        None
    };
    let proposal = store
        .update_skill_proposal_status(proposal_id, status)
        .await?;
    let mut payload = crate::dream::proposals::skill_proposal_payload(&proposal, status);
    if let Some(installation) = installation {
        payload["installation"] = serde_json::to_value(installation)?;
    }
    Ok(payload)
}

async fn apply_skill_rollback_effect(
    approval: &ApprovalRequestRecord,
    effect: &ApprovalEffectRecord,
    self_evolution_tier: SelfEvolutionTier,
    persona: &tm_modes::ModesConfig,
) -> Result<Value> {
    let rollback = effect
        .payload_json
        .get("rollback")
        .cloned()
        .ok_or_else(|| ServerError::Store("skill rollback effect missing rollback".to_string()))?;
    let name = rollback
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ServerError::Store("skill rollback effect missing name".to_string()))?;
    let expected_active_digest = rollback
        .get("expectedActiveDigest")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ServerError::Store("skill rollback effect missing expectedActiveDigest".to_string())
        })?;
    let target_digest = rollback
        .get("targetDigest")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ServerError::Store("skill rollback effect missing targetDigest".to_string())
        })?;
    let target_proposal_id = rollback
        .get("targetProposalId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            ServerError::Store("skill rollback effect missing targetProposalId".to_string())
        })?;
    let status = approval_terminal_status(&approval.status)?;
    let activation = if approval.status == "approved" {
        let digest = crate::evolution::evolution_content_digest(&rollback)?;
        crate::evolution::verify_approved_evolution_effect(
            self_evolution_tier,
            &effect.payload_json,
            tm_host::EvolutionTargetClass::SkillProposal,
            &target_proposal_id.to_string(),
            &digest,
        )?;
        let managed = persona
            .managed_skill(name)
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        let target = managed
            .versions
            .iter()
            .find(|version| version.content_digest == target_digest)
            .ok_or_else(|| {
                ServerError::NotFound(format!("managed skill {name} version {target_digest}"))
            })?;
        if target.source_proposal_id != target_proposal_id.to_string() {
            return Err(crate::evolution::policy_error(
                tm_host::EvolutionPolicyReason::InvalidPayload,
                format!("managed skill {name} rollback proposal provenance changed"),
            ));
        }
        Some(
            persona
                .rollback_managed_skill(name, expected_active_digest, target_digest)
                .map_err(|error| {
                    crate::evolution::policy_error(
                        tm_host::EvolutionPolicyReason::StaleApproval,
                        error.to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    Ok(
        crate::api::sessions::skill_lifecycle::skill_rollback_payload(
            &rollback,
            status,
            activation.as_ref(),
        ),
    )
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
    let mut activation = None;
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
        if current
            != (
                proposal.base_version,
                proposal.base_digest.clone(),
                proposal.base_active_digest.clone(),
            )
        {
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
        match (&proposal.apply_contract, &proposal.target) {
            (
                tm_modes::ReviewApplyContract::VersionedModeAddendum,
                tm_modes::ReviewProposalTarget::Mode { mode_id },
            ) => {
                activation = Some(
                    persona
                        .install_managed_mode_addendum(tm_modes::ManagedModeAddendumInstall {
                            mode_id: mode_id.clone(),
                            content_digest: proposal.content_digest.clone(),
                            base_version: proposal.base_version,
                            base_digest: proposal.base_digest.clone(),
                            source_proposal_id: proposal.id.to_string(),
                            expected_active_digest: proposal.base_active_digest.clone(),
                            changes: proposal.changes.clone(),
                        })
                        .map_err(|error| {
                            crate::evolution::policy_error(
                                tm_host::EvolutionPolicyReason::StaleApproval,
                                error.to_string(),
                            )
                        })?,
                );
            }
            (tm_modes::ReviewApplyContract::Disabled, _) => {}
            _ => {
                return Err(crate::evolution::policy_error(
                    tm_host::EvolutionPolicyReason::InvalidPayload,
                    format!(
                        "review proposal {proposal_id} apply contract does not match its target"
                    ),
                ));
            }
        }
    }
    let proposal = store
        .update_evolution_review_proposal_status(proposal_id, status)
        .await?;
    let mut payload =
        crate::api::sessions::evolution_review::evolution_review_proposal_payload(&proposal);
    if let Some(activation) = activation {
        payload["activation"] = serde_json::to_value(activation)?;
    }
    Ok(payload)
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

fn approval_terminal_status(status: &str) -> Result<&'static str> {
    match status {
        "approved" => Ok("approved"),
        "denied" => Ok("denied"),
        "timed_out" => Ok("timed_out"),
        "cancelled" => Ok("cancelled"),
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
