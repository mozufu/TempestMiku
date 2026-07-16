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

#[test]
fn hybrid_memory_context_preserves_scores_provenance_and_budget_decisions() {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let record = StoredMemoryRecord::new(MemoryRecordResource::Episodic(EpisodicMemoryRecord {
        schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
        id,
        owner_subject: "brian".to_string(),
        memory_scope: "project:tempestmiku".to_string(),
        text: "P8 hybrid recall enters the existing turn budgeter.".to_string(),
        evidence: vec![MemoryRecordEvidence::resource(
            "memory://evolution-proposals/example",
            "approved dream extraction",
        )],
        confidence: 0.9,
        importance: 0.8,
        observed_at: now,
        effective_from: now,
        effective_to: None,
        status: MemoryRecordStatus::Active,
        links: Default::default(),
        created_at: now,
    }))
    .unwrap();
    let context = MemoryContext::from_hybrid_candidates_with_profile_facts_and_summaries(
        "brian",
        "project:tempestmiku",
        vec![ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "global profile facts in project turns".to_string(),
            confidence: 0.9,
            importance: 0.8,
            provenance: "user-confirmed".to_string(),
            valid_from: now,
            valid_to: None,
        }],
        Vec::new(),
        vec![HybridMemoryCandidate {
            record,
            lexical_rank: Some(2),
            lexical_score: Some(0.7),
            dense_rank: Some(1),
            dense_score: Some(0.9),
            embedding_version: Some("emb-v1-fixture".to_string()),
            rrf_score: 0.0325,
        }],
        1_600,
        Some("emb-v1-fixture".to_string()),
    );

    assert_eq!(context.retrieval.mode, MemoryRetrievalMode::Hybrid);
    assert_eq!(context.profile_facts.len(), 1);
    assert_eq!(context.budget.included_profile_facts, 1);
    assert!(
        context.profile_facts[0]
            .text
            .contains("global profile facts in project turns")
    );
    assert_eq!(context.hybrid_recall.len(), 1);
    assert_eq!(context.budget.included_hybrid_recall, 1);
    assert!(context.retrieval.candidates[0].included);
    assert_eq!(context.retrieval.candidates[0].dense_rank, Some(1));
    assert_eq!(context.retrieval.candidates[0].lexical_rank, Some(2));
    assert!(
        context.retrieval.candidates[0]
            .source_uri
            .starts_with("memory://records/episodic/")
    );
    let rendered = context.render_prompt_block();
    assert!(rendered.contains("Hybrid episodic/semantic recall"));
    assert!(rendered.contains("approved dream extraction"));
    assert!(rendered.contains("rrf=0.032500"));
}

#[tokio::test]
async fn hybrid_memory_context_reserves_two_facts_two_ranked_recalls_and_one_summary() {
    let now = Utc::now();
    let store = InMemoryStore::default();
    let summary = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            title: "One bounded summary".to_string(),
            body: "Keep one summary visible beside facts and ranked recall.".to_string(),
            evidence: vec![MemoryEvidenceRef {
                session_id: Uuid::new_v4(),
                event_seq: Some(1),
                message_seq: None,
                uri: Some("memory://fixtures/summary".to_string()),
                label: "fixture".to_string(),
            }],
            source_dream_id: Uuid::new_v4(),
            source_session_id: None,
            dedupe_key: "summary:balanced-allocation".to_string(),
        })
        .await
        .unwrap();
    let facts = (0..5)
        .map(|index| ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: format!("preference-{index}"),
            object: format!("fact-{index}"),
            confidence: 0.9,
            importance: 0.8,
            provenance: "fixture".to_string(),
            valid_from: now,
            valid_to: None,
        })
        .collect::<Vec<_>>();
    let candidates = (0..5)
        .map(|index| {
            let id = Uuid::new_v4();
            let record =
                StoredMemoryRecord::new(MemoryRecordResource::Episodic(EpisodicMemoryRecord {
                    schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
                    id,
                    owner_subject: "brian".to_string(),
                    memory_scope: "global".to_string(),
                    text: format!("ranked recall {index}"),
                    evidence: vec![MemoryRecordEvidence::resource(
                        format!("memory://fixtures/ranked/{index}"),
                        "fixture",
                    )],
                    confidence: 0.9,
                    importance: 0.8,
                    observed_at: now,
                    effective_from: now,
                    effective_to: None,
                    status: MemoryRecordStatus::Active,
                    links: MemoryRecordLinks::default(),
                    created_at: now,
                }))
                .unwrap();
            HybridMemoryCandidate {
                record,
                lexical_rank: Some(index + 1),
                lexical_score: Some(1.0 / (index + 1) as f32),
                dense_rank: None,
                dense_score: None,
                embedding_version: None,
                rrf_score: 1.0 / (60 + index + 1) as f32,
            }
        })
        .collect::<Vec<_>>();

    let context = MemoryContext::from_hybrid_candidates_with_profile_facts_and_summaries(
        "brian",
        "global",
        facts,
        vec![summary],
        candidates,
        1_600,
        Some("fixture-version".to_string()),
    );

    assert_eq!(context.profile_facts.len(), 2);
    assert_eq!(context.hybrid_recall.len(), 2);
    assert_eq!(context.summaries.len(), 1);
    assert_eq!(
        context.profile_facts.len() + context.hybrid_recall.len() + context.summaries.len(),
        5
    );
    assert!(context.budget.truncated);
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

#[tokio::test]
async fn store_memory_provider_denies_tombstoned_legacy_scopes() {
    let store = Arc::new(InMemoryStore::default());
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:revoked".to_string(),
            text: "This legacy recall must disappear after unlink.".to_string(),
            source: "test:tombstone".to_string(),
            importance: 1.0,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    store
        .revoke_memory_scope("brian", "project:revoked", "linked folder removed")
        .await
        .unwrap();

    assert!(matches!(
        StoreMemoryProvider::new(store)
            .context_for_turn("brian", "project:revoked", "legacy recall")
            .await,
        Err(ServerError::NotFound(_))
    ));
}
