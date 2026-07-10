use std::sync::Arc;
use std::time::Duration as StdDuration;

use serde_json::{Value, json};
use uuid::Uuid;

use tm_memory::{
    DreamQueueRecord, MemoryEvidenceRef, NewSkillProposalRecord, SkillProposalRecord,
    SkillProposalStatus, SkillVerification,
};

use crate::memory::{MemoryWriteProposal, MemoryWriteStatus};
use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, CodingEventSink, DurableApprovalSpec, Result,
    Store, StoreCodingEventSink,
};

use super::util::{
    RedactedMessage, normalize_key, preview_text, reusable_workflow_signal, skill_name,
};
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

pub(super) fn dream_skill_proposal(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    evidence: &[MemoryEvidenceRef],
) -> Option<DreamSkillProposal> {
    let workflow_source = messages
        .iter()
        .find(|message| message.role == "user" && reusable_workflow_signal(&message.content))?;
    let scenario = preview_text(&workflow_source.content, 360);
    let name = skill_name(&scenario);
    let body = format!(
        "# {name}\n\nUse when Brian asks for the recurring workflow captured from this session.\n\n## Trigger\n{scenario}\n\n## Procedure\n- Restate the target outcome and scope.\n- Gather only missing constraints that affect the workflow.\n- Execute the smallest repeatable sequence of steps.\n- Preserve approvals for external, destructive, or sensitive actions.\n- End with the reusable result and any open loops.\n\n## Guardrails\n- Do not edit SOUL.md, mode catalogs, or capability configuration.\n- Do not install or activate automatically.\n"
    );
    let verification = verify_skill_body(&body);
    if !verification.passed {
        return Some(DreamSkillProposal::Rejected {
            name,
            scenario,
            reason: "generated skill proposal failed self-verification".to_string(),
            verification,
        });
    }
    Some(DreamSkillProposal::Accepted(NewSkillProposalRecord {
        name,
        description: "Reusable workflow distilled by a post-session dream.".to_string(),
        body,
        trigger: scenario.clone(),
        use_criteria: "Use only when the user asks for the same recurring workflow, not for one-off repo trivia.".to_string(),
        evidence: evidence.to_vec(),
        self_critique: "The proposal is intentionally narrow, cites the source session, and keeps live skill installation out of scope.".to_string(),
        verification,
        dedupe_key: format!(
            "skill:{}:{}",
            dream.session_id,
            normalize_key(&scenario)
        ),
        source_dream_id: dream.id,
        source_session_id: dream.session_id,
    }))
}

fn verify_skill_body(body: &str) -> SkillVerification {
    let checks = [
        ("has_title", body.starts_with("# ")),
        ("has_trigger", body.contains("## Trigger")),
        ("has_procedure", body.contains("## Procedure")),
        ("has_guardrails", body.contains("## Guardrails")),
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
    session_id: Uuid,
    proposal: MemoryWriteProposal,
    timeout: StdDuration,
) -> Result<()>
where
    S: Store,
{
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&store),
        sender_for(session_id),
    ));
    sink.emit(
        "write_proposal",
        proposal.event_payload(MemoryWriteStatus::Pending, None),
    )
    .await?;
    approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "memory".to_string(),
            prompt: memory_write_approval_prompt(&proposal, timeout),
            timeout,
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({ "proposal": proposal }),
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
) -> Result<()>
where
    S: Store,
{
    let session_id = proposal.source_session_id;
    let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
        session_id,
        Arc::clone(&store),
        sender_for(session_id),
    ));
    sink.emit(
        "write_proposal",
        skill_proposal_payload(&proposal, SkillProposalStatus::Pending),
    )
    .await?;
    approval_broker
        .enqueue_permission_for_backend(DurableApprovalSpec {
            session_id,
            origin: "skill".to_string(),
            prompt: skill_write_approval_prompt(&proposal, timeout),
            timeout,
            effect_type: "skill_write".to_string(),
            effect_payload_json: json!({ "proposalId": proposal.id }),
            resumable: true,
            sink,
        })
        .await?;
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
            proposal.text
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
        action: format!("skill.propose {}", proposal.name),
        scope: json!({
            "kind": "skill",
            "proposalId": proposal.id,
            "name": proposal.name,
            "description": proposal.description,
            "uri": skill_proposal_uri(proposal.id),
            "timeoutMs": timeout.as_millis(),
        }),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Accept proposal".to_string(),
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
    json!({
        "kind": "skill",
        "proposalId": proposal.id,
        "status": status,
        "name": proposal.name,
        "description": proposal.description,
        "trigger": proposal.trigger,
        "useCriteria": proposal.use_criteria,
        "selfCritique": proposal.self_critique,
        "verification": proposal.verification,
        "dedupeKey": proposal.dedupe_key,
        "sourceDreamId": proposal.source_dream_id,
        "sourceSessionId": proposal.source_session_id,
        "uri": skill_proposal_uri(proposal.id),
        "createdAt": proposal.created_at,
        "updatedAt": proposal.updated_at,
    })
}

fn skill_proposal_uri(id: Uuid) -> String {
    format!("memory://skill-proposals/{id}")
}
