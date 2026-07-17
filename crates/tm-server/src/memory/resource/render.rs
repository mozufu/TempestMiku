use tm_memory::{DreamQueueRecord, MemorySummaryRecord, SkillProposalRecord};

use crate::store::{ProfileFactRecord, RecallChunkRecord};

use super::super::MemoryContext;
use super::uri::{dream_uri, profile_fact_uri, recall_chunk_uri, skill_proposal_uri, summary_uri};

pub(super) fn render_dream_queue(scope: &str, dreams: &[DreamQueueRecord]) -> String {
    let mut lines = vec![
        "Dream queue".to_string(),
        format!("Scope: {scope}"),
        format!("Dreams: {}", dreams.len()),
    ];
    if dreams.is_empty() {
        lines.push("No dreams in this scope.".to_string());
    } else {
        lines.extend(dreams.iter().map(|dream| {
            format!(
                "- {} :: status={} reason={} session={} attempts={} available_at={} locked_at={} last_error={}",
                dream_uri(dream),
                dream.status,
                dream.reason,
                dream.session_id,
                dream.attempts,
                dream.available_at.to_rfc3339(),
                dream.locked_at
                    .map(|time| time.to_rfc3339())
                    .unwrap_or_else(|| "none".to_string()),
                dream.last_error.as_deref().unwrap_or("none")
            )
        }));
    }
    lines.join("\n")
}

pub(super) fn render_dream(dream: &DreamQueueRecord) -> String {
    [
        format!("Dream {}", dream.id),
        format!("URI: {}", dream_uri(dream)),
        format!("Session: {}", dream.session_id),
        format!("Subject: {}", dream.subject),
        format!("Scope: {}", dream.scope),
        format!("Reason: {}", dream.reason),
        format!("Status: {}", dream.status),
        format!("Dedupe key: {}", dream.dedupe_key),
        format!(
            "Source event seq: {}",
            dream
                .source_event_seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!("Attempts: {}", dream.attempts),
        format!("Enqueued at: {}", dream.enqueued_at.to_rfc3339()),
        format!("Available at: {}", dream.available_at.to_rfc3339()),
        format!(
            "Locked at: {}",
            dream
                .locked_at
                .map(|time| time.to_rfc3339())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "Last error: {}",
            dream.last_error.as_deref().unwrap_or("none")
        ),
    ]
    .join("\n")
}

pub(super) fn render_memory_root(
    subject: &str,
    scope: &str,
    context: &MemoryContext,
    facts: &[ProfileFactRecord],
    summaries: &[MemorySummaryRecord],
    chunks: &[RecallChunkRecord],
) -> String {
    let mut lines = vec![
        "Memory root".to_string(),
        format!("Subject: {subject}"),
        format!("Scope: {scope}"),
        format!("User model: memory://user-model"),
        format!(
            "Budget: {}/{} estimated tokens; profile facts: {}/{}; summaries: {}/{}; scoped recall: {}/{}; truncated: {}",
            context.budget.used_estimated_tokens,
            context.budget.max_tokens,
            context.budget.included_profile_facts,
            context.budget.available_profile_facts,
            context.budget.included_summaries,
            context.budget.available_summaries,
            context.budget.included_recall_chunks,
            context.budget.available_recall_chunks,
            context.budget.truncated
        ),
    ];
    if facts.is_empty() {
        lines.push("Profile facts: none".to_string());
    } else {
        lines.push("Profile facts:".to_string());
        lines.extend(facts.iter().map(|fact| {
            format!(
                "- {} :: {} {} {} (confidence {:.2}; importance {:.2}; provenance: {})",
                profile_fact_uri(fact),
                fact.subject,
                fact.predicate,
                fact.object,
                fact.confidence,
                fact.importance,
                fact.provenance
            )
        }));
    }
    if summaries.is_empty() {
        lines.push("Recent summaries: none".to_string());
    } else {
        lines.push("Recent summaries:".to_string());
        lines.extend(summaries.iter().map(|summary| {
            format!(
                "- {} :: {} summary: {} (source dream: {})",
                summary_uri(summary),
                summary.kind,
                summary.title,
                summary.source_dream_id
            )
        }));
    }
    if chunks.is_empty() {
        lines.push("Scoped recall chunks: none".to_string());
    } else {
        lines.push("Scoped recall chunks:".to_string());
        lines.extend(chunks.iter().map(|chunk| {
            format!(
                "- {} :: {} (source: {}; importance: {:.2})",
                recall_chunk_uri(chunk),
                chunk.text,
                chunk.source,
                chunk.importance
            )
        }));
    }
    lines.join("\n")
}

pub(super) fn render_user_model(subject: &str, facts: &[ProfileFactRecord]) -> String {
    let mut lines = vec![
        format!("User model"),
        format!("Subject: {subject}"),
        format!("Facts: {}", facts.len()),
    ];
    if facts.is_empty() {
        lines.push("No active profile facts.".to_string());
    } else {
        lines.extend(facts.iter().map(|fact| {
            format!(
                "- {} :: {} {} {} (confidence {:.2}; importance {:.2}; provenance: {}; valid from: {})",
                profile_fact_uri(fact),
                fact.subject,
                fact.predicate,
                fact.object,
                fact.confidence,
                fact.importance,
                fact.provenance,
                fact.valid_from.to_rfc3339()
            )
        }));
    }
    lines.join("\n")
}

pub(super) fn render_profile_fact(fact: &ProfileFactRecord) -> String {
    [
        format!("Profile fact {}", fact.id),
        format!("URI: {}", profile_fact_uri(fact)),
        format!("Subject: {}", fact.subject),
        format!("Predicate: {}", fact.predicate),
        format!("Object: {}", fact.object),
        format!("Confidence: {:.2}", fact.confidence),
        format!("Importance: {:.2}", fact.importance),
        format!("Provenance: {}", fact.provenance),
        format!("Valid from: {}", fact.valid_from.to_rfc3339()),
        format!(
            "Valid to: {}",
            fact.valid_to
                .map(|time| time.to_rfc3339())
                .unwrap_or_else(|| "active".to_string())
        ),
    ]
    .join("\n")
}

pub(super) fn render_recall_chunk(chunk: &RecallChunkRecord) -> String {
    [
        format!("Scoped recall chunk {}", chunk.id),
        format!("URI: {}", recall_chunk_uri(chunk)),
        format!("Scope: {}", chunk.scope),
        format!("Source: {}", chunk.source),
        format!("Importance: {:.2}", chunk.importance),
        format!("Created at: {}", chunk.created_at.to_rfc3339()),
        format!("Text: {}", chunk.text),
    ]
    .join("\n")
}

pub(super) fn render_summary(summary: &MemorySummaryRecord) -> String {
    let mut lines = vec![
        format!("Memory summary {}", summary.id),
        format!("URI: {}", summary_uri(summary)),
        format!("Kind: {}", summary.kind),
        format!("Subject: {}", summary.subject),
        format!("Scope: {}", summary.scope),
        format!("Title: {}", summary.title),
        format!("Source dream: {}", summary.source_dream_id),
        format!(
            "Source session: {}",
            summary
                .source_session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!("Updated at: {}", summary.updated_at.to_rfc3339()),
        "Evidence:".to_string(),
    ];
    if summary.evidence.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(summary.evidence.iter().map(|evidence| {
            format!(
                "- {} event={:?} message={:?} uri={}",
                evidence.label,
                evidence.event_seq,
                evidence.message_seq,
                evidence.uri.as_deref().unwrap_or("none")
            )
        }));
    }
    lines.push("Body:".to_string());
    lines.push(summary.body.clone());
    lines.join("\n")
}

pub(super) fn render_skill_proposal(proposal: &SkillProposalRecord) -> String {
    let lifecycle = tm_memory::skill_proposal_lifecycle(proposal);
    let mut lines = vec![
        format!("Skill proposal {}", proposal.id),
        format!("URI: {}", skill_proposal_uri(proposal)),
        format!("Name: {}", proposal.name),
        format!("Status: {}", proposal.status),
        format!("Normalized name: {}", lifecycle.normalized_name),
        format!("Candidate version: {}", lifecycle.version),
        format!("Content digest: {}", lifecycle.content_digest),
        format!("Reviewable: {}", lifecycle.reviewable),
        format!("Installable: {}", lifecycle.installable),
        format!("Conflict policy: {:?}", lifecycle.conflict_policy),
        format!("Rollback contract: {:?}", lifecycle.rollback),
        format!("Catalog reload: {:?}", lifecycle.catalog_reload),
        format!("Validation violations: {}", lifecycle.violations.join(", ")),
        format!("Description: {}", proposal.description),
        format!("Trigger: {}", proposal.trigger),
        format!("Use criteria: {}", proposal.use_criteria),
        format!("Source dream: {}", proposal.source_dream_id),
        format!("Source session: {}", proposal.source_session_id),
        format!("Verification passed: {}", proposal.verification.passed),
        format!(
            "Verification checks: {}",
            proposal.verification.checks.join(", ")
        ),
        format!("Self critique: {}", proposal.self_critique),
        "Evidence:".to_string(),
    ];
    if proposal.evidence.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(proposal.evidence.iter().map(|evidence| {
            format!(
                "- {} event={:?} message={:?} uri={}",
                evidence.label,
                evidence.event_seq,
                evidence.message_seq,
                evidence.uri.as_deref().unwrap_or("none")
            )
        }));
    }
    lines.push("Body:".to_string());
    lines.push(proposal.body.clone());
    lines.join("\n")
}
