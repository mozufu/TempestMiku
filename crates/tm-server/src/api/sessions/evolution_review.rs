use super::*;

use tm_host::EvolutionTargetClass;
use tm_modes::{
    ReviewAddendumChange, ReviewApplyContract, ReviewProposalStatus, ReviewProposalTarget,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposeEvolutionReviewRequest {
    pub target: ReviewProposalTarget,
    pub changes: Vec<ReviewAddendumChange>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionReviewProposalResponse {
    pub proposal_id: Uuid,
    pub approval_id: Uuid,
    pub status: ReviewProposalStatus,
    pub resource_uri: String,
    pub apply_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposeModeAddendumRollbackRequest {
    pub expected_active_digest: String,
    pub target_digest: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposePersonaAddendumRollbackRequest {
    pub expected_active_digest: String,
    pub target_digest: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeAddendumRollbackResponse {
    pub approval_id: Uuid,
    pub mode_id: tm_modes::ModeId,
    pub expected_active_digest: String,
    pub target_digest: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaAddendumRollbackResponse {
    pub approval_id: Uuid,
    pub persona_id: String,
    pub expected_active_digest: String,
    pub target_digest: Option<String>,
    pub status: String,
}

const PERSONA_AUTO_PROPOSAL_COOLDOWN_DAYS: i64 = 7;
const PERSONA_AUTO_PROPOSAL_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) async fn propose_evolution_review<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<ProposeEvolutionReviewRequest>,
) -> Result<Json<EvolutionReviewProposalResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let proposal_id = Uuid::new_v4();
    let target_class = review_target_class(&payload.target);
    let (base_version, base_digest, base_active_digest) =
        review_base_snapshot(&state.persona, &payload.target)?;
    let apply_contract = match &payload.target {
        ReviewProposalTarget::Persona { .. }
            if state.persona.managed_persona_addenda_path().is_some()
                && persona_changes_are_activatable(&payload.changes) =>
        {
            ReviewApplyContract::VersionedPersonaAddendum
        }
        ReviewProposalTarget::Mode { .. }
            if state.persona.managed_mode_addenda_path().is_some() =>
        {
            ReviewApplyContract::VersionedModeAddendum
        }
        _ => ReviewApplyContract::Disabled,
    };
    let content_digest = review_content_digest(
        &payload.target,
        base_version,
        &base_digest,
        &payload.changes,
    )?;
    let new = crate::NewEvolutionReviewProposal {
        id: proposal_id,
        session_id,
        target: payload.target,
        base_version,
        base_digest,
        base_active_digest,
        changes: payload.changes,
        content_digest,
        apply_contract,
        auto_candidate: None,
    };
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        target_class,
        proposal_id.to_string(),
        "owner",
        session_id,
        None,
        &review_content_value(&new),
    );
    let evolution = match evolution {
        Ok(evolution) => evolution,
        Err(error) => {
            let record = crate::evolution::denied_evolution_audit_record(
                crate::evolution::DeniedEvolutionAuditSpec {
                    tier: state.self_evolution_tier,
                    target_class,
                    target_id: proposal_id.to_string(),
                    actor_id: "owner".to_string(),
                    session_id,
                    dream_id: None,
                    content: &review_content_value(&new),
                    occurred_at: Utc::now(),
                },
            )?;
            state
                .store
                .append_evolution_audit(crate::EvolutionAuditEntry {
                    idempotency_key: format!("proposal:{proposal_id}:denied"),
                    record,
                })
                .await?;
            return Err(error);
        }
    };
    let proposal = state.store.create_evolution_review_proposal(new).await?;
    let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(60_000).clamp(1, 300_000));
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    sink.emit(
        "write_proposal",
        evolution_review_proposal_payload(&proposal),
    )
    .await?;
    let approval_id = state
        .approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "evolution-review".to_string(),
            prompt: evolution_review_approval_prompt(&proposal, timeout),
            timeout,
            effect_type: "evolution_review".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposalId": proposal.id,
            }),
            resumable: true,
            sink,
        })
        .await?;
    Ok(Json(EvolutionReviewProposalResponse {
        proposal_id,
        approval_id,
        status: ReviewProposalStatus::Pending,
        resource_uri: evolution_review_proposal_uri(proposal_id),
        apply_enabled: proposal.apply_contract != ReviewApplyContract::Disabled,
    }))
}

pub(super) async fn enqueue_auto_persona_candidate<S, M, C>(
    state: &AppState<S, M, C>,
    session_id: Uuid,
    mut candidate: super::persona_candidate::DetectedPersonaCandidate,
) -> Result<bool>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    if state.self_evolution_tier != tm_host::SelfEvolutionTier::Moderate
        || state.persona.managed_persona_addenda_path().is_none()
    {
        return Ok(false);
    }
    let target = ReviewProposalTarget::Persona {
        persona_id: "miku".to_string(),
    };
    let (base_version, base_digest, base_active_digest) =
        review_base_snapshot(&state.persona, &target)?;
    let active = state
        .persona
        .active_managed_persona_addendum("miku")
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    if active.as_ref().map(|active| active.content_digest.clone()) != base_active_digest {
        return Err(crate::evolution::policy_error(
            tm_host::EvolutionPolicyReason::StaleApproval,
            "managed persona addendum changed during auto-candidate snapshot",
        ));
    }
    if let Some(active) = &active {
        if active.changes.iter().any(|change| {
            change.section == candidate.change.section
                && change.after.summary == candidate.change.after.summary
        }) {
            return Ok(false);
        }
        candidate.change.before = active
            .changes
            .iter()
            .rev()
            .find(|change| change.section == candidate.change.section)
            .map(|change| change.after.clone());
    }

    let proposal_id = Uuid::new_v4();
    let changes = vec![candidate.change];
    let content_digest = review_content_digest(&target, base_version, &base_digest, &changes)?;
    let new = crate::NewEvolutionReviewProposal {
        id: proposal_id,
        session_id,
        target,
        base_version,
        base_digest,
        base_active_digest,
        changes,
        content_digest,
        apply_contract: ReviewApplyContract::VersionedPersonaAddendum,
        auto_candidate: Some(candidate.metadata),
    };
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        EvolutionTargetClass::PersonaProposal,
        proposal_id.to_string(),
        "auto-mode",
        session_id,
        None,
        &review_content_value(&new),
    )?;
    let created_at = Utc::now();
    let approval_id = Uuid::new_v4();
    let expires_at = created_at
        + chrono::Duration::from_std(PERSONA_AUTO_PROPOSAL_TIMEOUT)
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    let proposal_preview = crate::EvolutionReviewProposalRecord {
        id: new.id,
        session_id: new.session_id,
        target: new.target.clone(),
        base_version: new.base_version,
        base_digest: new.base_digest.clone(),
        base_active_digest: new.base_active_digest.clone(),
        changes: new.changes.clone(),
        content_digest: new.content_digest.clone(),
        status: ReviewProposalStatus::Pending,
        apply_contract: new.apply_contract,
        auto_candidate: new.auto_candidate.clone(),
        created_at,
        updated_at: created_at,
    };
    let prompt = evolution_review_approval_prompt(&proposal_preview, PERSONA_AUTO_PROPOSAL_TIMEOUT);
    let proposal_payload_json = evolution_review_proposal_payload(&proposal_preview);
    let approval_payload_json = json!({
        "approvalId": approval_id,
        "backend": "evolution-review",
        "action": prompt.action.clone(),
        "scope": prompt.scope.clone(),
        "options": prompt.options.clone(),
        "timeoutMs": PERSONA_AUTO_PROPOSAL_TIMEOUT.as_millis(),
        "expiresAt": expires_at,
        "resumable": true,
    });
    let created = state
        .store
        .create_auto_evolution_review_bundle(crate::NewAutoEvolutionReviewBundle {
            proposal: new,
            approval: crate::NewApprovalRequest {
                id: approval_id,
                session_id,
                turn_id: Some(candidate_source_turn_id(&proposal_preview)?),
                requester_id: state.approval_broker.requester_id(),
                origin: "evolution-review".to_string(),
                action: prompt.action,
                scope_json: prompt.scope,
                options_json: serde_json::to_value(prompt.options)?,
                effect_type: "evolution_review".to_string(),
                effect_payload_json: json!({
                    "evolution": evolution,
                    "proposalId": proposal_id,
                }),
                resumable: true,
                created_at,
                expires_at,
            },
            proposal_payload_json,
            approval_payload_json,
            cooldown_since: created_at
                - chrono::Duration::days(PERSONA_AUTO_PROPOSAL_COOLDOWN_DAYS),
        })
        .await?;
    if created.disposition != crate::AutoEvolutionReviewDisposition::Created {
        return Ok(false);
    }
    let proposal = created.proposal;
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::for_turn(
        session_id,
        proposal
            .auto_candidate
            .as_ref()
            .expect("created auto proposal has candidate metadata")
            .source_turn_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    if created.approval.is_none() || created.events.len() != 2 {
        return Err(ServerError::Store(
            "created auto evolution review bundle is incomplete".to_string(),
        ));
    }
    for event in created.events {
        sink.publish_persisted(event).await?;
    }
    Ok(true)
}

fn candidate_source_turn_id(proposal: &crate::EvolutionReviewProposalRecord) -> Result<Uuid> {
    proposal
        .auto_candidate
        .as_ref()
        .map(|candidate| candidate.source_turn_id)
        .ok_or_else(|| {
            ServerError::InvalidRequest(
                "auto evolution review proposal is missing candidate metadata".to_string(),
            )
        })
}

fn persona_changes_are_activatable(changes: &[ReviewAddendumChange]) -> bool {
    !changes.is_empty()
        && changes.iter().all(|change| {
            matches!(
                change.section,
                tm_modes::ReviewAddendumSection::ToneGuidance
                    | tm_modes::ReviewAddendumSection::AddressGuidance
                    | tm_modes::ReviewAddendumSection::InteractionPreference
            )
        })
}

pub(crate) fn review_target_class(target: &ReviewProposalTarget) -> EvolutionTargetClass {
    match target {
        ReviewProposalTarget::Persona { .. } => EvolutionTargetClass::PersonaProposal,
        ReviewProposalTarget::Mode { .. } => EvolutionTargetClass::ModeProposal,
    }
}

pub(crate) fn review_base_snapshot(
    persona: &tm_modes::ModesConfig,
    target: &ReviewProposalTarget,
) -> Result<(u64, String, Option<String>)> {
    let assets = persona.load_assets();
    let (value, base_active_digest) = match target {
        ReviewProposalTarget::Persona { persona_id } if persona_id == "miku" => {
            let active = if persona.managed_persona_addenda_path().is_some() {
                persona
                    .active_managed_persona_addendum(persona_id)
                    .map_err(|error| ServerError::InvalidRequest(error.to_string()))?
            } else {
                None
            };
            let active_digest = active.map(|version| version.content_digest);
            (
                json!({
                    "personaId": persona_id,
                    "soulDigest": crate::evolution::evolution_content_digest(&assets.soul)?.as_str(),
                    "modeCatalog": assets.modes,
                    "activeAddendumDigest": active_digest,
                }),
                active_digest,
            )
        }
        ReviewProposalTarget::Persona { persona_id } => {
            return Err(ServerError::InvalidRequest(format!(
                "unknown persona proposal target {persona_id}"
            )));
        }
        ReviewProposalTarget::Mode { mode_id } => {
            let profile = assets.modes.profile(mode_id).ok_or_else(|| {
                ServerError::InvalidRequest(format!("unknown mode proposal target {mode_id}"))
            })?;
            let active = if persona.managed_mode_addenda_path().is_some() {
                persona
                    .active_managed_mode_addendum(mode_id)
                    .map_err(|error| ServerError::InvalidRequest(error.to_string()))?
            } else {
                None
            };
            let active_digest = active.map(|version| version.content_digest);
            (
                json!({
                    "profile": profile,
                    "activeAddendumDigest": active_digest,
                }),
                active_digest,
            )
        }
    };
    Ok((
        1,
        crate::evolution::evolution_content_digest(&value)?
            .as_str()
            .to_string(),
        base_active_digest,
    ))
}

pub(crate) fn review_content_digest(
    target: &ReviewProposalTarget,
    base_version: u64,
    base_digest: &str,
    changes: &[ReviewAddendumChange],
) -> Result<String> {
    let mut changes = changes.to_vec();
    for change in &mut changes {
        change.after.label = tm_memory::redact_dream_text(&change.after.label).text;
        change.after.summary = tm_memory::redact_dream_text(&change.after.summary).text;
        if let Some(before) = &mut change.before {
            before.label = tm_memory::redact_dream_text(&before.label).text;
            before.summary = tm_memory::redact_dream_text(&before.summary).text;
        }
    }
    match target {
        ReviewProposalTarget::Mode { mode_id } => {
            tm_modes::mode_addendum_content_digest(mode_id, base_version, base_digest, &changes)
                .map_err(|error| ServerError::InvalidRequest(error.to_string()))
        }
        ReviewProposalTarget::Persona { persona_id } => tm_modes::persona_addendum_content_digest(
            persona_id,
            base_version,
            base_digest,
            &changes,
        )
        .map_err(|error| ServerError::InvalidRequest(error.to_string())),
    }
}

fn review_content_value(proposal: &crate::NewEvolutionReviewProposal) -> Value {
    json!({
        "target": proposal.target,
        "baseVersion": proposal.base_version,
        "baseDigest": proposal.base_digest,
        "changes": proposal.changes,
    })
}

pub(crate) fn evolution_review_proposal_payload(
    proposal: &crate::EvolutionReviewProposalRecord,
) -> Value {
    let preview = proposal
        .changes
        .iter()
        .map(|change| format!("{}: {}", change.after.label, change.after.summary))
        .collect::<Vec<_>>()
        .join("\n");
    let mut payload = json!({
        "kind": "evolution_review",
        "proposalId": proposal.id,
        "target": proposal.target,
        "status": proposal.status,
        "baseVersion": proposal.base_version,
        "baseDigest": proposal.base_digest,
        "preview": bounded_review_preview(&preview),
        "contentDigest": proposal.content_digest,
        "uri": evolution_review_proposal_uri(proposal.id),
        "applyEnabled": proposal.apply_contract != ReviewApplyContract::Disabled,
        "createdAt": proposal.created_at,
        "updatedAt": proposal.updated_at,
    });
    if let Some(candidate) = &proposal.auto_candidate {
        payload["source"] = json!("auto_mode");
        payload["candidateTrigger"] = json!(candidate.trigger);
        payload["evidenceCount"] = json!(candidate.evidence.len());
    }
    payload
}

fn evolution_review_approval_prompt(
    proposal: &crate::EvolutionReviewProposalRecord,
    timeout: Duration,
) -> ApprovalPrompt {
    let event = evolution_review_proposal_payload(proposal);
    let mut scope = json!({
        "kind": "evolution_review",
        "proposalId": proposal.id,
        "target": proposal.target,
        "baseVersion": proposal.base_version,
        "baseDigest": proposal.base_digest,
        "preview": event["preview"],
        "contentDigest": proposal.content_digest,
        "uri": evolution_review_proposal_uri(proposal.id),
        "applyEnabled": proposal.apply_contract != ReviewApplyContract::Disabled,
        "timeoutMs": timeout.as_millis(),
    });
    if let Some(candidate) = &proposal.auto_candidate {
        scope["source"] = json!("auto_mode");
        scope["candidateTrigger"] = json!(candidate.trigger);
        scope["evidenceCount"] = json!(candidate.evidence.len());
    }
    ApprovalPrompt {
        action: format!(
            "review {} addendum {}",
            proposal.target.kind(),
            proposal.target.id()
        ),
        scope,
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: match proposal.apply_contract {
                    ReviewApplyContract::Disabled => "Accept for review".to_string(),
                    ReviewApplyContract::VersionedModeAddendum => "Apply mode addendum".to_string(),
                    ReviewApplyContract::VersionedPersonaAddendum => {
                        "Apply persona addendum".to_string()
                    }
                },
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject proposal".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn bounded_review_preview(value: &str) -> String {
    let redacted = tm_memory::redact_dream_text(value).text;
    const MAX: usize = 512;
    if redacted.len() <= MAX {
        return redacted;
    }
    let mut end = MAX - '…'.len_utf8();
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &redacted[..end])
}

pub(crate) fn evolution_review_proposal_uri(id: Uuid) -> String {
    format!("memory://review-proposals/{id}")
}

pub(crate) async fn propose_mode_addendum_rollback<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, mode_id)): Path<(Uuid, String)>,
    Json(payload): Json<ProposeModeAddendumRollbackRequest>,
) -> Result<Json<ModeAddendumRollbackResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let mode_id = tm_modes::ModeId::new(mode_id);
    let managed = state
        .persona
        .managed_mode_addendum(&mode_id)
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    let active = managed.active.ok_or_else(|| {
        ServerError::InvalidRequest(format!("managed mode addendum {mode_id} is not active"))
    })?;
    if active.content_digest != payload.expected_active_digest {
        return Err(crate::evolution::policy_error(
            tm_host::EvolutionPolicyReason::StaleApproval,
            format!(
                "managed mode addendum {mode_id} active version changed from {} to {}",
                payload.expected_active_digest, active.content_digest
            ),
        ));
    }
    if payload.target_digest.as_deref() == Some(payload.expected_active_digest.as_str()) {
        return Err(ServerError::InvalidRequest(format!(
            "managed mode addendum {mode_id} is already at {}",
            payload.expected_active_digest
        )));
    }
    let target_proposal_id = payload
        .target_digest
        .as_ref()
        .map(|digest| {
            managed
                .versions
                .iter()
                .find(|version| &version.content_digest == digest)
                .ok_or_else(|| {
                    ServerError::NotFound(format!(
                        "managed mode addendum {mode_id} version {digest}"
                    ))
                })
                .and_then(|version| {
                    Uuid::parse_str(&version.source_proposal_id).map_err(|_| {
                        ServerError::Store(format!(
                            "managed mode addendum {mode_id} target version has invalid source proposal id"
                        ))
                    })
                })
        })
        .transpose()?;
    let evolution_target_id = target_proposal_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| active.source_proposal_id.clone());
    let rollback = json!({
        "modeId": mode_id,
        "expectedActiveDigest": payload.expected_active_digest,
        "targetDigest": payload.target_digest,
        "targetProposalId": target_proposal_id,
        "evolutionTargetId": evolution_target_id,
    });
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        EvolutionTargetClass::ModeProposal,
        evolution_target_id,
        "owner",
        session_id,
        None,
        &rollback,
    )?;
    let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(60_000).clamp(1, 300_000));
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    sink.emit(
        "write_proposal",
        mode_addendum_rollback_payload(&rollback, "pending", None),
    )
    .await?;
    let approval_id = state
        .approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "mode-addendum-rollback".to_string(),
            prompt: mode_addendum_rollback_prompt(&rollback, timeout),
            timeout,
            effect_type: "mode_addendum_rollback".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "rollback": rollback,
            }),
            resumable: true,
            sink,
        })
        .await?;
    Ok(Json(ModeAddendumRollbackResponse {
        approval_id,
        mode_id,
        expected_active_digest: payload.expected_active_digest,
        target_digest: payload.target_digest,
        status: "pending".to_string(),
    }))
}

pub(crate) async fn propose_persona_addendum_rollback<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path((session_id, persona_id)): Path<(Uuid, String)>,
    Json(payload): Json<ProposePersonaAddendumRollbackRequest>,
) -> Result<Json<PersonaAddendumRollbackResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.store.get_session(session_id).await?;
    let managed = state
        .persona
        .managed_persona_addendum(&persona_id)
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    let active = managed.active.ok_or_else(|| {
        ServerError::InvalidRequest(format!(
            "managed persona addendum {persona_id} is not active"
        ))
    })?;
    if active.content_digest != payload.expected_active_digest {
        return Err(crate::evolution::policy_error(
            tm_host::EvolutionPolicyReason::StaleApproval,
            format!(
                "managed persona addendum {persona_id} active version changed from {} to {}",
                payload.expected_active_digest, active.content_digest
            ),
        ));
    }
    if payload.target_digest.as_deref() == Some(payload.expected_active_digest.as_str()) {
        return Err(ServerError::InvalidRequest(format!(
            "managed persona addendum {persona_id} is already at {}",
            payload.expected_active_digest
        )));
    }
    let target_proposal_id = payload
        .target_digest
        .as_ref()
        .map(|digest| {
            managed
                .versions
                .iter()
                .find(|version| &version.content_digest == digest)
                .ok_or_else(|| {
                    ServerError::NotFound(format!(
                        "managed persona addendum {persona_id} version {digest}"
                    ))
                })
                .and_then(|version| {
                    Uuid::parse_str(&version.source_proposal_id).map_err(|_| {
                        ServerError::Store(format!(
                            "managed persona addendum {persona_id} target version has invalid source proposal id"
                        ))
                    })
                })
        })
        .transpose()?;
    let evolution_target_id = target_proposal_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| active.source_proposal_id.clone());
    let rollback = json!({
        "personaId": persona_id,
        "expectedActiveDigest": payload.expected_active_digest,
        "targetDigest": payload.target_digest,
        "targetProposalId": target_proposal_id,
        "evolutionTargetId": evolution_target_id,
    });
    let evolution = crate::evolution::evolution_effect_metadata(
        state.self_evolution_tier,
        EvolutionTargetClass::PersonaProposal,
        evolution_target_id,
        "owner",
        session_id,
        None,
        &rollback,
    )?;
    let timeout = Duration::from_millis(payload.timeout_ms.unwrap_or(60_000).clamp(1, 300_000));
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&state.store),
        state.sender(session_id),
    ));
    sink.emit(
        "write_proposal",
        persona_addendum_rollback_payload(&rollback, "pending", None),
    )
    .await?;
    let approval_id = state
        .approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "persona-addendum-rollback".to_string(),
            prompt: persona_addendum_rollback_prompt(&rollback, timeout),
            timeout,
            effect_type: "persona_addendum_rollback".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "rollback": rollback,
            }),
            resumable: true,
            sink,
        })
        .await?;
    Ok(Json(PersonaAddendumRollbackResponse {
        approval_id,
        persona_id,
        expected_active_digest: payload.expected_active_digest,
        target_digest: payload.target_digest,
        status: "pending".to_string(),
    }))
}

pub(crate) fn persona_addendum_rollback_payload(
    rollback: &Value,
    status: &str,
    activation: Option<&tm_modes::ManagedPersonaAddendumActivation>,
) -> Value {
    let mut payload = json!({
        "kind": "persona_addendum_rollback",
        "status": status,
        "personaId": rollback["personaId"],
        "expectedActiveDigest": rollback["expectedActiveDigest"],
        "targetDigest": rollback["targetDigest"],
        "targetProposalId": rollback["targetProposalId"],
    });
    if let Some(activation) = activation {
        payload["activation"] = serde_json::to_value(activation).unwrap_or(Value::Null);
    }
    payload
}

fn persona_addendum_rollback_prompt(rollback: &Value, timeout: Duration) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!(
            "persona.addendum.rollback {}",
            rollback["personaId"].as_str().unwrap_or_default()
        ),
        scope: json!({
            "kind": "persona_addendum_rollback",
            "personaId": rollback["personaId"],
            "expectedActiveDigest": rollback["expectedActiveDigest"],
            "targetDigest": rollback["targetDigest"],
            "targetProposalId": rollback["targetProposalId"],
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Roll back persona guidance".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Keep current persona guidance".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

pub(crate) fn mode_addendum_rollback_payload(
    rollback: &Value,
    status: &str,
    activation: Option<&tm_modes::ManagedModeAddendumActivation>,
) -> Value {
    let mut payload = json!({
        "kind": "mode_addendum_rollback",
        "status": status,
        "modeId": rollback["modeId"],
        "expectedActiveDigest": rollback["expectedActiveDigest"],
        "targetDigest": rollback["targetDigest"],
        "targetProposalId": rollback["targetProposalId"],
    });
    if let Some(activation) = activation {
        payload["activation"] = serde_json::to_value(activation).unwrap_or(Value::Null);
    }
    payload
}

fn mode_addendum_rollback_prompt(rollback: &Value, timeout: Duration) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!(
            "mode.addendum.rollback {}",
            rollback["modeId"].as_str().unwrap_or_default()
        ),
        scope: json!({
            "kind": "mode_addendum_rollback",
            "modeId": rollback["modeId"],
            "expectedActiveDigest": rollback["expectedActiveDigest"],
            "targetDigest": rollback["targetDigest"],
            "targetProposalId": rollback["targetProposalId"],
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Roll back mode guidance".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Keep current mode guidance".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}
