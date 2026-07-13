use super::*;

use tm_host::EvolutionTargetClass;
use tm_modes::{ReviewAddendumChange, ReviewProposalStatus, ReviewProposalTarget};

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
    let (base_version, base_digest) = review_base_snapshot(&state.persona, &payload.target)?;
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
        changes: payload.changes,
        content_digest,
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
        apply_enabled: false,
    }))
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
) -> Result<(u64, String)> {
    let assets = persona.load_assets();
    let value = match target {
        ReviewProposalTarget::Persona { persona_id } if persona_id == "miku" => json!({
            "personaId": persona_id,
            "soulDigest": crate::evolution::evolution_content_digest(&assets.soul)?.as_str(),
            "modeCatalog": assets.modes,
        }),
        ReviewProposalTarget::Persona { persona_id } => {
            return Err(ServerError::InvalidRequest(format!(
                "unknown persona proposal target {persona_id}"
            )));
        }
        ReviewProposalTarget::Mode { mode_id } => {
            let profile = assets.modes.profile(mode_id).ok_or_else(|| {
                ServerError::InvalidRequest(format!("unknown mode proposal target {mode_id}"))
            })?;
            serde_json::to_value(profile)?
        }
    };
    Ok((
        1,
        crate::evolution::evolution_content_digest(&value)?
            .as_str()
            .to_string(),
    ))
}

pub(crate) fn review_content_digest(
    target: &ReviewProposalTarget,
    base_version: u64,
    base_digest: &str,
    changes: &[ReviewAddendumChange],
) -> Result<String> {
    Ok(crate::evolution::evolution_content_digest(&json!({
        "target": target,
        "baseVersion": base_version,
        "baseDigest": base_digest,
        "changes": changes,
    }))?
    .as_str()
    .to_string())
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
    json!({
        "kind": "evolution_review",
        "proposalId": proposal.id,
        "target": proposal.target,
        "status": proposal.status,
        "baseVersion": proposal.base_version,
        "baseDigest": proposal.base_digest,
        "preview": bounded_review_preview(&preview),
        "contentDigest": proposal.content_digest,
        "uri": evolution_review_proposal_uri(proposal.id),
        "applyEnabled": false,
        "createdAt": proposal.created_at,
        "updatedAt": proposal.updated_at,
    })
}

fn evolution_review_approval_prompt(
    proposal: &crate::EvolutionReviewProposalRecord,
    timeout: Duration,
) -> ApprovalPrompt {
    let event = evolution_review_proposal_payload(proposal);
    ApprovalPrompt {
        action: format!(
            "review {} addendum {}",
            proposal.target.kind(),
            proposal.target.id()
        ),
        scope: json!({
            "kind": "evolution_review",
            "proposalId": proposal.id,
            "target": proposal.target,
            "baseVersion": proposal.base_version,
            "baseDigest": proposal.base_digest,
            "preview": event["preview"],
            "contentDigest": proposal.content_digest,
            "uri": evolution_review_proposal_uri(proposal.id),
            "applyEnabled": false,
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Accept for review".to_string(),
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
