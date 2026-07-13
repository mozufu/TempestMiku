use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_host::{
    EVOLUTION_WIRE_VERSION, EvolutionActorId, EvolutionAuditRecord, EvolutionAuditStatus,
    EvolutionDigest, EvolutionOrigin, EvolutionPolicyDecision, EvolutionPolicyOutcome,
    EvolutionPolicyReason, EvolutionTarget, EvolutionTargetClass, EvolutionTargetId,
    SelfEvolutionTier, decide_evolution_target,
};
use uuid::Uuid;

use crate::{MemoryWriteKind, Result, ServerError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvolutionEffectMetadata {
    pub version: u16,
    pub configured_tier: SelfEvolutionTier,
    pub decision: EvolutionPolicyDecision,
    pub origin: EvolutionOrigin,
    pub target: EvolutionTarget,
    pub content_digest: EvolutionDigest,
}

pub(crate) fn ensure_evolution_proposal_reachable(
    tier: SelfEvolutionTier,
    target: EvolutionTargetClass,
) -> Result<EvolutionPolicyDecision> {
    let decision = decide_evolution_target(tier, target);
    if decision.outcome == EvolutionPolicyOutcome::Denied {
        return Err(policy_error(
            decision
                .reason
                .unwrap_or(EvolutionPolicyReason::InsufficientTier),
            format!("tier {tier} cannot propose target {}", target.as_str()),
        ));
    }
    Ok(decision)
}

pub(crate) const fn memory_target_class(kind: MemoryWriteKind) -> EvolutionTargetClass {
    match kind {
        MemoryWriteKind::ProfileFact => EvolutionTargetClass::ProfileFact,
        MemoryWriteKind::RecallChunk => EvolutionTargetClass::ScopedMemory,
    }
}

pub(crate) fn evolution_effect_metadata(
    tier: SelfEvolutionTier,
    target_class: EvolutionTargetClass,
    target_id: impl Into<String>,
    actor_id: impl Into<String>,
    session_id: uuid::Uuid,
    dream_id: Option<uuid::Uuid>,
    content: &impl Serialize,
) -> Result<EvolutionEffectMetadata> {
    let decision = ensure_evolution_proposal_reachable(tier, target_class)?;
    let target_id = EvolutionTargetId::new(target_id)
        .map_err(|error| invalid_payload(format!("invalid target id: {error}")))?;
    let actor_id = EvolutionActorId::new(actor_id)
        .map_err(|error| invalid_payload(format!("invalid actor id: {error}")))?;
    Ok(EvolutionEffectMetadata {
        version: EVOLUTION_WIRE_VERSION,
        configured_tier: tier,
        decision,
        origin: EvolutionOrigin {
            actor_id,
            session_id,
            dream_id,
        },
        target: EvolutionTarget {
            class: target_class,
            id: target_id,
        },
        content_digest: evolution_content_digest(content)?,
    })
}

pub(crate) fn evolution_audit_record(
    payload: &Value,
    approval_id: Uuid,
    effect_id: Uuid,
    status: EvolutionAuditStatus,
    occurred_at: chrono::DateTime<chrono::Utc>,
    error_code: Option<EvolutionPolicyReason>,
) -> Result<Option<EvolutionAuditRecord>> {
    let Some(metadata) = payload.get("evolution") else {
        return Ok(None);
    };
    let metadata: EvolutionEffectMetadata = serde_json::from_value(metadata.clone())
        .map_err(|error| invalid_payload(format!("invalid evolution metadata: {error}")))?;
    if metadata.version != EVOLUTION_WIRE_VERSION {
        return Err(invalid_payload(format!(
            "unsupported evolution effect version {}",
            metadata.version
        )));
    }
    let proposal_id = Uuid::parse_str(metadata.target.id.as_str())
        .map_err(|error| invalid_payload(format!("target id is not a proposal UUID: {error}")))?;
    Ok(Some(EvolutionAuditRecord {
        version: metadata.version,
        id: Uuid::new_v4(),
        proposal_id,
        origin: metadata.origin,
        target: metadata.target,
        content_digest: metadata.content_digest,
        configured_tier: metadata.configured_tier,
        decision: metadata.decision,
        approval_id: Some(approval_id),
        effect_id: Some(effect_id),
        status,
        created_at: occurred_at,
        updated_at: occurred_at,
        error_code,
    }))
}

pub(crate) struct DeniedEvolutionAuditSpec<'a, T> {
    pub tier: SelfEvolutionTier,
    pub target_class: EvolutionTargetClass,
    pub target_id: String,
    pub actor_id: String,
    pub session_id: Uuid,
    pub dream_id: Option<Uuid>,
    pub content: &'a T,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) fn denied_evolution_audit_record<T: Serialize>(
    spec: DeniedEvolutionAuditSpec<'_, T>,
) -> Result<EvolutionAuditRecord> {
    let target_id = EvolutionTargetId::new(spec.target_id)
        .map_err(|error| invalid_payload(format!("invalid target id: {error}")))?;
    let proposal_id = Uuid::parse_str(target_id.as_str())
        .map_err(|error| invalid_payload(format!("target id is not a proposal UUID: {error}")))?;
    let actor_id = EvolutionActorId::new(spec.actor_id)
        .map_err(|error| invalid_payload(format!("invalid actor id: {error}")))?;
    let decision = decide_evolution_target(spec.tier, spec.target_class);
    if decision.outcome != EvolutionPolicyOutcome::Denied {
        return Err(invalid_payload(format!(
            "tier {} does not deny target {}",
            spec.tier,
            spec.target_class.as_str()
        )));
    }
    Ok(EvolutionAuditRecord {
        version: EVOLUTION_WIRE_VERSION,
        id: Uuid::new_v4(),
        proposal_id,
        origin: EvolutionOrigin {
            actor_id,
            session_id: spec.session_id,
            dream_id: spec.dream_id,
        },
        target: EvolutionTarget {
            class: spec.target_class,
            id: target_id,
        },
        content_digest: evolution_content_digest(spec.content)?,
        configured_tier: spec.tier,
        decision,
        approval_id: None,
        effect_id: None,
        status: EvolutionAuditStatus::Denied,
        created_at: spec.occurred_at,
        updated_at: spec.occurred_at,
        error_code: decision.reason,
    })
}

pub(crate) fn evolution_policy_error_code(error: &str) -> Option<EvolutionPolicyReason> {
    [
        EvolutionPolicyReason::DisabledTier,
        EvolutionPolicyReason::InsufficientTier,
        EvolutionPolicyReason::UnsupportedAggressive,
        EvolutionPolicyReason::UnknownTarget,
        EvolutionPolicyReason::ApprovalRequired,
        EvolutionPolicyReason::StaleApproval,
        EvolutionPolicyReason::InvalidPayload,
    ]
    .into_iter()
    .find(|reason| error.contains(reason.code()))
}

pub(crate) fn audit_status_from_approval(status: &str) -> Result<EvolutionAuditStatus> {
    match status {
        "approved" => Ok(EvolutionAuditStatus::Approved),
        "denied" => Ok(EvolutionAuditStatus::Denied),
        "timed_out" => Ok(EvolutionAuditStatus::TimedOut),
        "cancelled" => Ok(EvolutionAuditStatus::Superseded),
        other => Err(invalid_payload(format!(
            "unsupported approval audit status {other}"
        ))),
    }
}

pub(crate) fn evolution_content_digest(content: &impl Serialize) -> Result<EvolutionDigest> {
    let mut value = serde_json::to_value(content)
        .map_err(|error| invalid_payload(format!("candidate serialization failed: {error}")))?;
    tm_memory::redact_json_value(&mut value);
    canonicalize_json(&mut value);
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| invalid_payload(format!("candidate encoding failed: {error}")))?;
    EvolutionDigest::new(format!("sha256:{:x}", Sha256::digest(encoded)))
        .map_err(|error| invalid_payload(format!("candidate digest failed: {error}")))
}

fn canonicalize_json(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(canonicalize_json),
        Value::Object(map) => {
            let mut entries = std::mem::take(map).into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            for (key, mut value) in entries {
                canonicalize_json(&mut value);
                map.insert(key, value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub(crate) fn deterministic_evolution_proposal_id(kind: &str, key: &str) -> Uuid {
    let digest = Sha256::digest(format!("tempestmiku:evolution:{kind}:{key}"));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

pub(crate) fn verify_approved_evolution_effect(
    current_tier: SelfEvolutionTier,
    payload: &Value,
    expected_class: EvolutionTargetClass,
    expected_id: &str,
    expected_digest: &EvolutionDigest,
) -> Result<()> {
    verify_evolution_effect(
        current_tier,
        payload,
        expected_class,
        expected_id,
        expected_digest,
        false,
    )
}

pub(crate) fn verify_approved_review_effect(
    current_tier: SelfEvolutionTier,
    payload: &Value,
    expected_class: EvolutionTargetClass,
    expected_id: &str,
    expected_digest: &EvolutionDigest,
) -> Result<()> {
    verify_evolution_effect(
        current_tier,
        payload,
        expected_class,
        expected_id,
        expected_digest,
        true,
    )
}

fn verify_evolution_effect(
    current_tier: SelfEvolutionTier,
    payload: &Value,
    expected_class: EvolutionTargetClass,
    expected_id: &str,
    expected_digest: &EvolutionDigest,
    review_only: bool,
) -> Result<()> {
    let metadata: EvolutionEffectMetadata = serde_json::from_value(
        payload
            .get("evolution")
            .cloned()
            .ok_or_else(|| invalid_payload("effect is missing evolution metadata"))?,
    )
    .map_err(|error| invalid_payload(format!("invalid evolution metadata: {error}")))?;
    if metadata.version != EVOLUTION_WIRE_VERSION {
        return Err(invalid_payload(format!(
            "unsupported evolution effect version {}",
            metadata.version
        )));
    }
    if metadata.target.class != expected_class || metadata.target.id.as_str() != expected_id {
        return Err(invalid_payload(format!(
            "effect target does not match payload-derived {}:{expected_id}",
            expected_class.as_str()
        )));
    }
    if &metadata.content_digest != expected_digest {
        return Err(invalid_payload(
            "effect content digest does not match payload-derived candidate",
        ));
    }

    let creation_decision = decide_evolution_target(metadata.configured_tier, expected_class);
    if creation_decision.outcome == EvolutionPolicyOutcome::Denied {
        return Err(invalid_payload(format!(
            "effect was created under tier {} without target authority",
            metadata.configured_tier
        )));
    }

    let current_decision = decide_evolution_target(current_tier, expected_class);
    if current_decision.outcome != EvolutionPolicyOutcome::Allowed
        && !(review_only && current_decision.outcome == EvolutionPolicyOutcome::ApprovalRequired)
    {
        return Err(policy_error(
            current_decision
                .reason
                .unwrap_or(EvolutionPolicyReason::InsufficientTier),
            format!(
                "tier {current_tier} cannot apply target {}",
                expected_class.as_str()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn policy_error(reason: EvolutionPolicyReason, detail: impl AsRef<str>) -> ServerError {
    ServerError::Policy(format!("{}: {}", reason.code(), detail.as_ref()))
}

fn invalid_payload(detail: impl AsRef<str>) -> ServerError {
    policy_error(EvolutionPolicyReason::InvalidPayload, detail)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn content_digest_is_stable_across_json_object_key_order() {
        let left = json!({"outer": {"z": 1, "a": 2}, "tail": true});
        let right = json!({"tail": true, "outer": {"a": 2, "z": 1}});

        assert_eq!(
            evolution_content_digest(&left).unwrap(),
            evolution_content_digest(&right).unwrap()
        );
    }

    #[test]
    fn effect_verification_rechecks_current_tier_and_payload_target() {
        let id = "00000000-0000-0000-0000-000000000000";
        let metadata = evolution_effect_metadata(
            SelfEvolutionTier::Conservative,
            EvolutionTargetClass::ProfileFact,
            id,
            "test-actor",
            uuid::Uuid::nil(),
            None,
            &json!({"candidate": "value"}),
        )
        .unwrap();
        let digest = metadata.content_digest.clone();
        let payload = json!({ "evolution": metadata });
        verify_approved_evolution_effect(
            SelfEvolutionTier::Conservative,
            &payload,
            EvolutionTargetClass::ProfileFact,
            id,
            &digest,
        )
        .unwrap();

        let downgraded = verify_approved_evolution_effect(
            SelfEvolutionTier::Off,
            &payload,
            EvolutionTargetClass::ProfileFact,
            id,
            &digest,
        )
        .unwrap_err();
        assert!(downgraded.to_string().contains("evolution_disabled"));

        let forged = verify_approved_evolution_effect(
            SelfEvolutionTier::Conservative,
            &payload,
            EvolutionTargetClass::ScopedMemory,
            id,
            &digest,
        )
        .unwrap_err();
        assert!(forged.to_string().contains("evolution_invalid_payload"));
    }

    #[test]
    fn effect_verification_rejects_missing_unknown_and_unauthorized_metadata() {
        let missing = verify_approved_evolution_effect(
            SelfEvolutionTier::Conservative,
            &json!({}),
            EvolutionTargetClass::SkillProposal,
            "skill-proposal",
            &EvolutionDigest::new(format!("sha256:{}", "0".repeat(64))).unwrap(),
        )
        .unwrap_err();
        assert!(missing.to_string().contains("evolution_invalid_payload"));

        let aggressive = json!({
            "evolution": {
                "version": EVOLUTION_WIRE_VERSION,
                "configuredTier": "aggressive",
                "origin": {
                    "actorId": "test-actor",
                    "sessionId": uuid::Uuid::nil()
                },
                "target": { "class": "skill_proposal", "id": "skill-proposal" },
                "contentDigest": format!("sha256:{}", "0".repeat(64))
            }
        });
        assert!(
            verify_approved_evolution_effect(
                SelfEvolutionTier::Conservative,
                &aggressive,
                EvolutionTargetClass::SkillProposal,
                "skill-proposal",
                &EvolutionDigest::new(format!("sha256:{}", "0".repeat(64))).unwrap(),
            )
            .unwrap_err()
            .to_string()
            .contains("evolution_invalid_payload")
        );

        let unauthorized = json!({
            "evolution": {
                "version": EVOLUTION_WIRE_VERSION,
                "configuredTier": "off",
                "origin": {
                    "actorId": "test-actor",
                    "sessionId": uuid::Uuid::nil()
                },
                "target": { "class": "skill_proposal", "id": "skill-proposal" },
                "contentDigest": format!("sha256:{}", "0".repeat(64))
            }
        });
        assert!(
            verify_approved_evolution_effect(
                SelfEvolutionTier::Conservative,
                &unauthorized,
                EvolutionTargetClass::SkillProposal,
                "skill-proposal",
                &EvolutionDigest::new(format!("sha256:{}", "0".repeat(64))).unwrap(),
            )
            .unwrap_err()
            .to_string()
            .contains("evolution_invalid_payload")
        );
    }
}
