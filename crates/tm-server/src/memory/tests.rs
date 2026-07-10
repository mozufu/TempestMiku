use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use tm_memory::{MemoryEvidenceRef, MemorySummaryKind, NewMemorySummaryRecord};
use tm_modes::{AssetStatus, ModeId};
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};
use crate::{InMemoryStore, NewSession, Store};

use super::util::short_id;
use super::*;

#[test]
fn memory_context_renders_provenance_and_budget_metadata() {
    let fact_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let context = MemoryContext::from_records(
        "brian",
        "project:tempestmiku",
        vec![ProfileFactRecord {
            id: fact_id,
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "boring Rust".to_string(),
            confidence: 0.9,
            importance: 0.72,
            provenance: "memory://turns/1".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        }],
        vec![RecallChunkRecord {
            id: chunk_id,
            scope: "project:tempestmiku".to_string(),
            text: "Keep approval writes replayable.".to_string(),
            source: "session:abc:assistant".to_string(),
            importance: 0.78,
            created_at: Utc::now(),
        }],
        1_600,
    );

    let rendered = context.render_prompt_block();
    assert!(rendered.contains("budget:"));
    assert!(rendered.contains("profile facts: 1/1"));
    assert!(rendered.contains("scoped recall: 1/1"));
    assert!(rendered.contains("importance 0.72"));
    assert!(rendered.contains("importance 0.78"));
    assert!(rendered.contains("provenance: memory://turns/1"));
    assert!(rendered.contains("scope: project:tempestmiku"));
    assert!(rendered.contains("boring Rust"));
    assert_eq!(context.profile_facts[0].id, fact_id);
    assert_eq!(context.recall_chunks[0].id, chunk_id);
}

#[test]
fn memory_context_trims_to_budget_and_reports_truncation() {
    let context = MemoryContext::from_records(
        "brian",
        "global",
        vec![ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "a very long durable fact that will not fit the tiny prompt budget".to_string(),
            confidence: 0.9,
            importance: 0.72,
            provenance: "test".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        }],
        Vec::new(),
        4,
    );

    assert!(context.budget.truncated);
    assert_eq!(context.budget.included_profile_facts, 0);
    assert!(
        context
            .render_prompt_block()
            .contains("No memory items fit")
    );
}

#[tokio::test]
async fn store_memory_provider_includes_recent_summary_with_provenance_and_budget() {
    let store = Arc::new(InMemoryStore::default());
    let source_session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let source_dream_id = Uuid::new_v4();
    let summary = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "project:tempestmiku".to_string(),
            title: "Release notes cleanup".to_string(),
            body: "Open loops: update the changelog.\nNext likely action: run the web smoke."
                .to_string(),
            evidence: vec![MemoryEvidenceRef {
                session_id: source_session.id,
                event_seq: Some(7),
                message_seq: None,
                uri: Some("artifact://release-notes".to_string()),
                label: "artifact".to_string(),
            }],
            source_dream_id,
            source_session_id: Some(source_session.id),
            dedupe_key: format!("summary:test:{}", source_session.id),
        })
        .await
        .unwrap();

    let provider = StoreMemoryProvider::new(Arc::clone(&store))
        .with_summary_limit(3)
        .with_prompt_budget_tokens(240);
    let context = provider
        .context_for_turn("brian", "project:tempestmiku", "what is next")
        .await
        .unwrap();

    assert_eq!(context.summaries.len(), 1);
    assert_eq!(context.budget.available_summaries, 1);
    assert_eq!(context.budget.included_summaries, 1);
    let rendered = context.render_prompt_block();
    assert!(rendered.contains("summaries: 1/1"));
    assert!(rendered.contains(&format!("memory://summaries/{}", summary.id)));
    assert!(rendered.contains("Open loops: update the changelog"));
    assert!(rendered.contains("Next likely action: run the web smoke"));
    assert!(rendered.contains(&format!("dream:{}", short_id(source_dream_id))));

    let tiny = StoreMemoryProvider::new(store)
        .with_summary_limit(3)
        .with_prompt_budget_tokens(8)
        .context_for_turn("brian", "project:tempestmiku", "what is next")
        .await
        .unwrap();
    assert_eq!(tiny.budget.available_summaries, 1);
    assert_eq!(tiny.budget.included_summaries, 0);
    assert!(tiny.budget.truncated);
    assert!(tiny.render_prompt_block().contains("No memory items fit"));
}

#[tokio::test]
async fn store_memory_provider_uses_lexical_importance_recency_and_degrades_empty() {
    let store = Arc::new(InMemoryStore::default());
    let now = Utc::now();
    let high_id = Uuid::new_v4();
    let low_id = Uuid::new_v4();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: low_id,
            scope: "project:tempestmiku".to_string(),
            text: "Ledger follow-up: newer but less important".to_string(),
            source: "test:newer".to_string(),
            importance: 0.4,
            created_at: now,
        })
        .await
        .unwrap();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: high_id,
            scope: "project:tempestmiku".to_string(),
            text: "Ledger follow-up: older but critical".to_string(),
            source: "test:older".to_string(),
            importance: 0.9,
            created_at: now - chrono::Duration::days(1),
        })
        .await
        .unwrap();

    let context = StoreMemoryProvider::new(Arc::clone(&store))
        .context_for_turn("brian", "project:tempestmiku", "ledger")
        .await
        .unwrap();
    assert_eq!(context.recall_chunks.len(), 2);
    assert_eq!(context.recall_chunks[0].id, high_id);
    assert_eq!(context.recall_chunks[1].id, low_id);
    assert!(context.render_prompt_block().contains("importance 0.90"));

    let empty = StoreMemoryProvider::new(store)
        .context_for_turn("brian", "project:missing", "ledger")
        .await
        .unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.budget.available_recall_chunks, 0);
    assert_eq!(empty.budget.available_summaries, 0);
    assert!(!empty.budget.truncated);
}

#[test]
fn memory_write_proposals_use_stable_record_ids_for_idempotency() {
    let a = MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "Remember the same thing".to_string(),
        "session:a".to_string(),
        "manual".to_string(),
        json!({ "source": "test" }),
        Utc::now(),
    )
    .unwrap();
    let b = MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "  remember   the SAME thing ".to_string(),
        "session:b".to_string(),
        "manual".to_string(),
        json!({ "source": "test" }),
        Utc::now(),
    )
    .unwrap();

    assert_eq!(a.dedupe_key, b.dedupe_key);
    assert_eq!(a.record_id, b.record_id);
    assert_ne!(a.proposal_id, b.proposal_id);
}

#[test]
fn personal_assistant_state_capture_captures_durable_state_categories() {
    let proposals = personal_assistant_state_capture_proposals(
        "brian",
        "project:tempestmiku",
        Uuid::new_v4(),
        "Remember that I prefer short approval summaries.\n\
             Active project: TempestMiku P2.5 state capture.\n\
             Deadline: send the P2 notes by Friday.\n\
             Decision: keep memory writes approval-backed.\n\
             Shipped: artifact://0 has the capture fixture.\n\
             Workflow: for release notes, gather commits then draft concise bullets.",
        Utc::now(),
    )
    .unwrap();

    assert_eq!(proposals.len(), 6);
    let fact = proposals
        .iter()
        .find(|proposal| proposal.memory_kind == MemoryWriteKind::ProfileFact)
        .unwrap();
    assert_eq!(fact.predicate.as_deref(), Some("prefers"));
    assert_eq!(fact.object.as_deref(), Some("short approval summaries"));
    assert_eq!(fact.provenance_label, STATE_CAPTURE_PROVENANCE_LABEL);
    assert_eq!(fact.importance_score, 0.72);
    assert_eq!(fact.provenance["importanceScore"], json!(0.72));

    let recall_proposals = proposals
        .iter()
        .filter(|proposal| proposal.memory_kind == MemoryWriteKind::RecallChunk)
        .collect::<Vec<_>>();
    let recall_text = recall_proposals
        .iter()
        .map(|proposal| proposal.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recall_text.contains("Open loop: Active project: TempestMiku P2.5 state capture"));
    assert!(recall_text.contains("Commitment/deadline: Deadline: send the P2 notes by Friday"));
    assert!(recall_text.contains("Decision: Decision: keep memory writes approval-backed"));
    assert!(
        recall_text.contains("Shipped artifact: Shipped: artifact://0 has the capture fixture")
    );
    assert!(recall_text.contains("Reusable workflow: Workflow: for release notes"));
    assert!(recall_proposals.iter().all(|proposal| {
        (proposal.provenance["importanceScore"].as_f64().unwrap()
            - proposal.importance_score as f64)
            .abs()
            < 0.01
    }));
    assert!(
        recall_proposals
            .iter()
            .any(|proposal| proposal.importance_score >= 0.86)
    );
}

#[test]
fn personal_assistant_state_capture_captures_bounded_reminders_as_recall() {
    let proposals = personal_assistant_state_capture_proposals(
        "brian",
        "global",
        Uuid::new_v4(),
        "Remind me to review the P2 acceptance checklist by Friday.\n\
             Don't let me forget to update ROADMAP after tests pass.",
        Utc::now(),
    )
    .unwrap();

    assert_eq!(proposals.len(), 2);
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.memory_kind == MemoryWriteKind::RecallChunk)
    );
    let recall_text = proposals
        .iter()
        .map(|proposal| proposal.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recall_text.contains("Reminder: review the P2 acceptance checklist by Friday"));
    assert!(recall_text.contains("Reminder: update ROADMAP after tests pass"));
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.provenance["capturedCategory"] == "personal_reminder")
    );
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.importance_score == 0.64)
    );
}

#[test]
fn personal_assistant_state_capture_does_not_capture_noise_or_sensitive_content() {
    for content in [
        "I'm overwhelmed and sad tonight.",
        "Just venting: that meeting was annoying.",
        "Please remember my password is hunter2.",
        "Reminder: rotate sk-testsecret123456 tomorrow.",
        "Remember to contact brian@example.com after release.",
        "Remember my passport number is X1234567.",
        "2026-07-03 ERROR failed to connect\n2026-07-03 WARN retrying\nstack backtrace:",
        "```text\nINFO raw logs should stay out of memory\nERROR nope\n```",
    ] {
        let proposals = personal_assistant_state_capture_proposals(
            "brian",
            "global",
            Uuid::new_v4(),
            content,
            Utc::now(),
        )
        .unwrap();
        assert!(proposals.is_empty(), "{content}");
    }
}
