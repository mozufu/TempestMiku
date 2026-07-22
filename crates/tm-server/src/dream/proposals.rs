use std::sync::Arc;
use std::time::Duration as StdDuration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use tm_host::{EvolutionTargetClass, SelfEvolutionTier};
use tm_memory::{
    DreamQueueRecord, EvolutionPolicyRecord, MemoryEvidenceRef, NewSkillProposalRecord,
    SkillProposalRecord, SkillProposalStatus, SkillVerification,
};

use crate::memory::{MemoryWriteProposal, MemoryWriteStatus};
use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, CodingEventSink, DurableApprovalSpec,
    NewApprovalRequest, NewSkillApprovalBundle, Result, Store, StoreCodingEventSink,
};

pub(super) struct MemoryProposalContext {
    pub session_id: Uuid,
    pub dream_id: Uuid,
    pub self_evolution_tier: SelfEvolutionTier,
}

use super::util::skill_name;
use super::worker::SenderFactory;

pub(super) enum DreamSkillProposal {
    Accepted(NewSkillProposalRecord),
    Rejected {
        name: String,
        scenario: String,
        reason: String,
        verification: SkillVerification,
    },
}

pub(super) fn policy_skill_proposal(
    dream: &DreamQueueRecord,
    policy: &EvolutionPolicyRecord,
    evidence: &[MemoryEvidenceRef],
) -> Option<DreamSkillProposal> {
    if evidence.is_empty() {
        return None;
    }
    let name = skill_name(&policy.trigger);
    let evidence_lines = evidence
        .iter()
        .filter_map(|reference| {
            reference.uri.as_ref().map(|uri| match reference.event_seq {
                Some(seq) => format!("- {uri} (event {seq})"),
                None => format!("- {uri}"),
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    if evidence_lines.is_empty() {
        return None;
    }
    let body = format!(
        "# {name}\n\n{}\n\n## Trigger\n{}\n\n## Applicability\n{}\n\n## Procedure\n{}\n\n## Verification\n{}\n\n## Guardrails\n- Do not edit SOUL.md, mode catalogs, or capability configuration.\n- Do not install or activate automatically.\n- Stop and fall back to first-principles investigation when the Applicability boundary does not match.\n\n## Evidence\n{evidence_lines}\n",
        policy.trigger, policy.trigger, policy.boundary, policy.procedure, policy.verification,
    );
    let verification = verify_skill_body(&body);
    if !verification.passed {
        return Some(DreamSkillProposal::Rejected {
            name,
            scenario: policy.trigger.clone(),
            reason: "generated skill proposal failed self-verification".to_string(),
            verification,
        });
    }
    let revision_digest = Sha256::digest(
        serde_json::to_vec(&json!({
            "name": name,
            "body": body,
            "trigger": policy.trigger,
            "useCriteria": policy.boundary,
            "evidence": evidence,
            "estimatedGain": policy.gain,
            "supportEpisodes": policy.support_episode_ids,
        }))
        .expect("skill proposal revision fields are serializable"),
    );
    let revision_digest = format!("sha256:{revision_digest:x}");
    Some(DreamSkillProposal::Accepted(NewSkillProposalRecord {
        name,
        description: "Evidence-grounded procedure crystallized from a durable evolution policy."
            .to_string(),
        body,
        trigger: policy.trigger.clone(),
        use_criteria: policy.boundary.clone(),
        evidence: evidence.to_vec(),
        self_critique: "The proposal is bounded by its policy applicability, preserves evidence links, and requires explicit approval before installation.".to_string(),
        verification,
        dedupe_key: format!(
            "skill:policy:{}:{}:{revision_digest}",
            policy.id, policy.version
        ),
        source_dream_id: dream.id,
        source_session_id: dream.session_id,
        source_policy_id: Some(policy.id),
        estimated_gain: Some(policy.gain),
        support_episodes: u32::try_from(policy.support_episode_ids.len()).unwrap_or(u32::MAX),
    }))
}

fn verify_skill_body(body: &str) -> SkillVerification {
    let checks = [
        ("has_title", body.starts_with("# ")),
        ("has_trigger", body.contains("## Trigger")),
        ("has_applicability", body.contains("## Applicability")),
        ("has_procedure", body.contains("## Procedure")),
        ("has_verification", body.contains("## Verification")),
        ("has_guardrails", body.contains("## Guardrails")),
        (
            "has_evidence",
            body.contains("memory://evolution/episodes/"),
        ),
        ("does_not_mutate_identity", !body.contains("write SOUL.md")),
        (
            "does_not_claim_live_reload",
            !body.contains("Install automatically") && !body.contains("Activate automatically"),
        ),
    ];
    SkillVerification {
        passed: checks.iter().all(|(_, passed)| *passed),
        checks: checks
            .into_iter()
            .map(|(name, passed)| format!("{name}:{}", if passed { "pass" } else { "fail" }))
            .collect(),
    }
}

pub(super) async fn spawn_memory_write_proposal<S>(
    store: Arc<S>,
    approval_broker: Arc<ApprovalBroker>,
    sender_for: SenderFactory,
    proposal: MemoryWriteProposal,
    timeout: StdDuration,
    context: MemoryProposalContext,
) -> Result<()>
where
    S: Store,
{
    let evolution = crate::evolution::evolution_effect_metadata(
        context.self_evolution_tier,
        crate::evolution::memory_target_class(proposal.memory_kind),
        proposal.proposal_id.to_string(),
        "dream-worker",
        context.session_id,
        Some(context.dream_id),
        &proposal,
    )?;
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        context.session_id,
        Arc::clone(&store),
        sender_for(context.session_id),
    ));
    sink.emit(
        "write_proposal",
        proposal.event_payload(MemoryWriteStatus::Pending, None),
    )
    .await?;
    approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id: context.session_id,
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

pub(super) async fn spawn_skill_write_proposal<S>(
    store: Arc<S>,
    approval_broker: Arc<ApprovalBroker>,
    sender_for: SenderFactory,
    proposal: SkillProposalRecord,
    timeout: StdDuration,
    self_evolution_tier: SelfEvolutionTier,
) -> Result<()>
where
    S: Store,
{
    let evolution = crate::evolution::evolution_effect_metadata(
        self_evolution_tier,
        EvolutionTargetClass::SkillProposal,
        proposal.id.to_string(),
        "dream-worker",
        proposal.source_session_id,
        Some(proposal.source_dream_id),
        &proposal,
    )?;
    let session_id = proposal.source_session_id;
    let prompt = skill_write_approval_prompt(&proposal, timeout);
    let approval_id = Uuid::new_v4();
    let created_at = chrono::Utc::now();
    let expires_at = created_at
        + chrono::Duration::from_std(timeout)
            .map_err(|error| crate::ServerError::InvalidRequest(error.to_string()))?;
    let approval_payload_json = json!({
        "approvalId": approval_id,
        "backend": "skill",
        "action": prompt.action,
        "scope": prompt.scope,
        "options": prompt.options,
        "timeoutMs": timeout.as_millis(),
        "expiresAt": expires_at,
        "resumable": true,
    });
    let result = store
        .create_skill_approval_bundle(NewSkillApprovalBundle {
            proposal: proposal.clone(),
            approval: NewApprovalRequest {
                id: approval_id,
                session_id,
                turn_id: None,
                requester_id: approval_broker.requester_id(),
                origin: "skill".to_string(),
                action: prompt.action,
                scope_json: prompt.scope,
                options_json: serde_json::to_value(&prompt.options)?,
                effect_type: "skill_write".to_string(),
                effect_payload_json: json!({
                    "evolution": evolution,
                    "proposalId": proposal.id,
                }),
                resumable: true,
                created_at,
                expires_at,
            },
            proposal_payload_json: skill_proposal_payload(&proposal, SkillProposalStatus::Pending),
            approval_payload_json,
        })
        .await?;
    if !result.events.is_empty() {
        let sender = sender_for(session_id);
        for event in result.events {
            let _ = sender.send(event);
        }
    }
    Ok(())
}

fn memory_write_approval_prompt(
    proposal: &MemoryWriteProposal,
    timeout: StdDuration,
) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!(
            "memory.write {}: {}",
            proposal.memory_kind.as_str(),
            proposal.preview_text()
        ),
        scope: json!({
            "proposal": proposal.approval_scope(),
            "timeoutMs": timeout.as_millis(),
            "source": "dream",
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Save memory".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject memory".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

fn skill_write_approval_prompt(
    proposal: &SkillProposalRecord,
    timeout: StdDuration,
) -> ApprovalPrompt {
    ApprovalPrompt {
        action: format!("skill.install {}", proposal.name),
        scope: json!({
            "kind": "skill",
            "proposalId": proposal.id,
            "name": proposal.name,
            "preview": bounded_preview(&proposal.description, 512),
            "uri": skill_proposal_uri(proposal.id),
            "contentDigest": skill_content_digest(proposal),
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Install skill".to_string(),
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

pub(crate) fn skill_proposal_payload(
    proposal: &SkillProposalRecord,
    status: SkillProposalStatus,
) -> Value {
    let lifecycle = tm_memory::skill_proposal_lifecycle(proposal);
    json!({
        "kind": "skill",
        "proposalId": proposal.id,
        "status": status,
        "name": proposal.name,
        "preview": bounded_preview(&proposal.description, 512),
        "contentDigest": lifecycle.content_digest,
        "installEnabled": lifecycle.installable,
        "catalogReload": lifecycle.catalog_reload,
        "rollback": lifecycle.rollback,
        "sourceDreamId": proposal.source_dream_id,
        "sourceSessionId": proposal.source_session_id,
        "uri": skill_proposal_uri(proposal.id),
        "createdAt": proposal.created_at,
        "updatedAt": proposal.updated_at,
    })
}

fn skill_content_digest(proposal: &SkillProposalRecord) -> String {
    tm_memory::skill_proposal_lifecycle(proposal).content_digest
}

fn bounded_preview(value: &str, max_bytes: usize) -> String {
    let redacted = tm_memory::redact_dream_text(value).text;
    if redacted.len() <= max_bytes {
        return redacted;
    }
    let mut end = max_bytes.saturating_sub('…'.len_utf8());
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &redacted[..end])
}

fn skill_proposal_uri(id: Uuid) -> String {
    format!("memory://skill-proposals/{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tm_memory::{DreamReason, DreamStatus, PolicyStatus};

    #[test]
    fn policy_proposal_preserves_evidence_and_applicability() {
        let now = Utc::now();
        let dream = DreamQueueRecord {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            status: DreamStatus::Running,
            dedupe_key: "dream:test".to_string(),
            source_event_seq: None,
            attempts: 1,
            enqueued_at: now,
            available_at: now,
            locked_at: Some(now),
            lease_owner: Some(Uuid::new_v4()),
            lease_epoch: 1,
            heartbeat_at: Some(now),
            completed_at: None,
            error_at: None,
            last_error: None,
        };
        let policy = EvolutionPolicyRecord {
            id: Uuid::new_v4(),
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            signature: "fs|enoent".to_string(),
            trigger: "Recurring safe release notes work".to_string(),
            procedure: "- inspect evidence\n- draft notes".to_string(),
            verification: "The draft cites the checked changes.".to_string(),
            boundary: "Only for release notes from verified repository evidence.".to_string(),
            support_episode_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            gain: 0.42,
            status: PolicyStatus::Active,
            version: 3,
            created_at: now,
            updated_at: now,
        };
        let episode_id = policy.support_episode_ids[0];
        let evidence = vec![MemoryEvidenceRef {
            session_id: dream.session_id,
            event_seq: Some(17),
            message_seq: None,
            uri: Some(format!("memory://evolution/episodes/{episode_id}")),
            label: "positive trace".to_string(),
        }];

        let DreamSkillProposal::Accepted(proposal) =
            policy_skill_proposal(&dream, &policy, &evidence).expect("eligible proposal")
        else {
            panic!("valid policy proposal rejected")
        };
        assert_eq!(proposal.source_policy_id, Some(policy.id));
        assert_eq!(proposal.estimated_gain, Some(policy.gain));
        assert_eq!(proposal.support_episodes, 2);
        assert!(proposal.body.contains("## Applicability"));
        assert!(proposal.body.contains("## Verification"));
        assert!(proposal.body.contains(&format!(
            "memory://evolution/episodes/{episode_id} (event 17)"
        )));
        assert!(proposal.verification.passed);
    }

    #[test]
    fn policy_proposal_requires_grounding_evidence() {
        let now = Utc::now();
        let dream = DreamQueueRecord {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            status: DreamStatus::Running,
            dedupe_key: "dream:test-empty".to_string(),
            source_event_seq: None,
            attempts: 1,
            enqueued_at: now,
            available_at: now,
            locked_at: Some(now),
            lease_owner: Some(Uuid::new_v4()),
            lease_epoch: 1,
            heartbeat_at: Some(now),
            completed_at: None,
            error_at: None,
            last_error: None,
        };
        let policy = EvolutionPolicyRecord {
            id: Uuid::new_v4(),
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            signature: "fs|ok".to_string(),
            trigger: "Recurring work".to_string(),
            procedure: "- inspect".to_string(),
            verification: "Checked result.".to_string(),
            boundary: "Global only.".to_string(),
            support_episode_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            gain: 0.2,
            status: PolicyStatus::Active,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        assert!(policy_skill_proposal(&dream, &policy, &[]).is_none());
    }
}
