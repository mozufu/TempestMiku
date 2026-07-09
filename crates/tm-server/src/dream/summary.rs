use chrono::Utc;
use serde_json::json;
use tm_memory::{
    BudgetedDreamInput, DreamQueueRecord, MemoryEvidenceRef, MemorySummaryKind,
    MemorySummaryRecord, NewMemorySummaryRecord,
};

use crate::memory::MemoryWriteProposal;
use crate::{Result, SessionEvent, Store};

use super::util::{
    RedactedMessage, event_uris, message_lines_with, preview_text, unresolved_approvals,
};

pub(super) fn evidence_refs(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    events: &[SessionEvent],
) -> Vec<MemoryEvidenceRef> {
    let mut evidence = Vec::new();
    if let Some(seq) = dream.source_event_seq {
        evidence.push(MemoryEvidenceRef {
            session_id: dream.session_id,
            event_seq: Some(seq),
            message_seq: None,
            uri: None,
            label: "session_end".to_string(),
        });
    }
    evidence.extend(messages.iter().take(8).map(|message| MemoryEvidenceRef {
        session_id: dream.session_id,
        event_seq: None,
        message_seq: Some(message.seq),
        uri: None,
        label: format!("message:{}", message.role),
    }));
    evidence.extend(
        events
            .iter()
            .filter_map(|event| {
                event
                    .artifact_uri
                    .as_ref()
                    .or(event.history_uri.as_ref())
                    .map(|uri| (event, uri))
            })
            .take(8)
            .map(|(event, uri)| MemoryEvidenceRef {
                session_id: dream.session_id,
                event_seq: Some(event.seq),
                message_seq: None,
                uri: Some(uri.clone()),
                label: event.event_type.clone(),
            }),
    );
    evidence
}

pub(super) fn bounded_summary(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    events: &[SessionEvent],
    input: &BudgetedDreamInput,
    max_chars: usize,
) -> String {
    let first_user = messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| preview_text(&message.content, 280))
        .unwrap_or_else(|| "No user message was captured.".to_string());
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| preview_text(&message.content, 360))
        .unwrap_or_else(|| "No assistant final response was captured.".to_string());
    let artifacts = event_uris(events);
    let decisions = message_lines_with(messages, &["decision:", "decided", "going with"]);
    let open_loops = message_lines_with(messages, &["todo:", "open loop", "follow up", "next"]);
    let unresolved = unresolved_approvals(events);
    let mut lines = vec![
        "Session summary".to_string(),
        format!("Subject: {}", dream.subject),
        format!("Scope: {}", dream.scope),
        format!("Source session: {}", dream.session_id),
        format!(
            "Input budget: included {}/{} messages across {} chunk(s); truncated messages: {}; omitted messages: {}; included chars: {}/{}.",
            input.included_messages,
            input.total_messages,
            input.chunks.len(),
            input.truncated_messages,
            input.omitted_messages,
            input.included_chars,
            input.total_chars
        ),
        format!("What happened: {first_user}"),
        format!("Assistant result: {last_assistant}"),
        format!(
            "Shipped artifacts/resources: {}",
            if artifacts.is_empty() {
                "none observed".to_string()
            } else {
                artifacts.join(", ")
            }
        ),
        format!(
            "Decisions: {}",
            if decisions.is_empty() {
                "none explicit".to_string()
            } else {
                decisions.join(" | ")
            }
        ),
        format!(
            "Open loops: {}",
            if open_loops.is_empty() {
                "none explicit".to_string()
            } else {
                open_loops.join(" | ")
            }
        ),
        format!(
            "Unresolved approvals: {}",
            if unresolved.is_empty() {
                "none".to_string()
            } else {
                unresolved.join(", ")
            }
        ),
    ];
    lines.push(if open_loops.is_empty() && unresolved.is_empty() {
        "Next likely action: continue from the latest assistant result if Brian resumes this scope."
            .to_string()
    } else {
        "Next likely action: surface the open loops or pending approvals before taking new action."
            .to_string()
    });
    preview_text(&lines.join("\n"), max_chars)
}

pub(super) fn summary_title(messages: &[RedactedMessage]) -> String {
    messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| preview_text(&message.content, 80))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Post-session dream summary".to_string())
}

pub(super) fn dream_memory_proposals(
    dream: &DreamQueueRecord,
    messages: &[RedactedMessage],
    evidence: &[MemoryEvidenceRef],
) -> Result<Vec<MemoryWriteProposal>> {
    let mut proposals = Vec::new();
    for message in messages
        .iter()
        .filter(|message| message.role == "user" && !message.had_redactions)
    {
        let mut extracted = crate::memory::personal_assistant_state_capture_proposals(
            &dream.subject,
            &dream.scope,
            dream.session_id,
            &message.content,
            Utc::now(),
        )?;
        for proposal in &mut extracted {
            proposal.source = format!("dream:{}:session:{}", dream.id, dream.session_id);
            proposal.provenance_label = "post-session-dream".to_string();
            proposal.provenance = json!({
                "label": "post-session-dream",
                "sourceDream": dream.id,
                "sourceSession": dream.session_id,
                "sourceMessageSeq": message.seq,
                "scope": dream.scope,
                "subject": dream.subject,
                "evidence": evidence,
            });
        }
        proposals.extend(extracted);
    }
    Ok(proposals)
}

pub(super) async fn write_reflection_summary_if_needed<S>(
    store: &S,
    dream: &DreamQueueRecord,
    proposals: &[MemoryWriteProposal],
    evidence: &[MemoryEvidenceRef],
    title_seed: &str,
    threshold: f32,
    max_chars: usize,
) -> Result<Option<MemorySummaryRecord>>
where
    S: Store,
{
    let cumulative_importance = proposals
        .iter()
        .map(|proposal| proposal.importance_score)
        .sum::<f32>();
    if proposals.is_empty() || cumulative_importance < threshold {
        return Ok(None);
    }

    let mut lines = vec![
        "Reflection summary".to_string(),
        format!("Scope: {}", dream.scope),
        format!("Source dream: {}", dream.id),
        format!(
            "Cumulative importance: {:.2} (threshold {:.2}).",
            cumulative_importance, threshold
        ),
        "Signals:".to_string(),
    ];
    lines.extend(proposals.iter().take(6).map(|proposal| {
        format!(
            "- {:.2} {} :: {}",
            proposal.importance_score,
            proposal.memory_kind.as_str(),
            preview_text(&proposal.text, 180)
        )
    }));
    lines.push("Evidence citations:".to_string());
    lines.extend(evidence.iter().take(6).map(evidence_citation));
    lines.push(
        "Derived reflection: preserve the durable decision/open-loop context before the next turn, \
         and prefer recalling the cited memory records over raw session logs."
            .to_string(),
    );

    let record = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Reflection,
            subject: dream.subject.clone(),
            scope: dream.scope.clone(),
            title: format!("Reflection: {}", preview_text(title_seed, 72)),
            body: preview_text(&lines.join("\n"), max_chars),
            evidence: evidence.to_vec(),
            source_dream_id: dream.id,
            source_session_id: Some(dream.session_id),
            dedupe_key: format!("summary:reflection:{}", dream.session_id),
        })
        .await?;
    Ok(Some(record))
}

pub(super) async fn update_recursive_summary_rollup<S>(
    store: &S,
    dream: &DreamQueueRecord,
    max_chars: usize,
) -> Result<Option<MemorySummaryRecord>>
where
    S: Store,
{
    let mut summaries = store.memory_summaries(&dream.scope, 12).await?;
    summaries.retain(|summary| {
        matches!(
            summary.kind,
            MemorySummaryKind::Session
                | MemorySummaryKind::Reflection
                | MemorySummaryKind::Daily
                | MemorySummaryKind::Weekly
        )
    });
    if summaries.is_empty() {
        return Ok(None);
    }
    summaries.sort_by_key(|summary| summary.updated_at);
    let folded = summaries.into_iter().rev().take(6).collect::<Vec<_>>();
    let mut chronological = folded.clone();
    chronological.reverse();

    let mut lines = vec![
        "Recursive scope summary".to_string(),
        format!("Scope: {}", dream.scope),
        format!("Folded summaries: {}.", chronological.len()),
    ];
    lines.extend(chronological.iter().map(|summary| {
        format!(
            "- [{}] {} :: {}",
            summary.kind,
            summary.title,
            preview_text(&summary.body, 260)
        )
    }));
    lines.push(
        "Next recall rule: use this rollup to recover older session context before loading raw logs."
            .to_string(),
    );
    let evidence = chronological
        .iter()
        .map(|summary| MemoryEvidenceRef {
            session_id: summary.source_session_id.unwrap_or(dream.session_id),
            event_seq: None,
            message_seq: None,
            uri: Some(summary_uri(summary)),
            label: format!("summary:{}", summary.kind),
        })
        .collect::<Vec<_>>();

    let record = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::TopicProject,
            subject: dream.subject.clone(),
            scope: dream.scope.clone(),
            title: format!("Rollup: {}", dream.scope),
            body: preview_text(&lines.join("\n"), max_chars),
            evidence,
            source_dream_id: dream.id,
            source_session_id: None,
            dedupe_key: format!("summary:rollup:{}", dream.scope),
        })
        .await?;
    Ok(Some(record))
}

fn evidence_citation(evidence: &MemoryEvidenceRef) -> String {
    let mut parts = vec![format!("session {}", evidence.session_id)];
    if let Some(seq) = evidence.event_seq {
        parts.push(format!("event {seq}"));
    }
    if let Some(seq) = evidence.message_seq {
        parts.push(format!("message {seq}"));
    }
    if let Some(uri) = evidence.uri.as_ref() {
        parts.push(uri.clone());
    }
    format!("- {} ({})", evidence.label, parts.join(", "))
}

pub(super) fn summary_uri(summary: &MemorySummaryRecord) -> String {
    format!("memory://summaries/{}", summary.id)
}