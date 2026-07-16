use super::*;

#[tokio::test]
async fn p8_1_evaluator_replays_in_memory_and_serializes_record_contracts() {
    let manifest = RecallEvaluationManifest::parse(P8_RECALL_MANIFEST_JSON).unwrap();
    let store = InMemoryStore::default();
    seed_p8_recall_fixture(&store, &manifest).await;

    let report = evaluate_lexical_recall_baseline(
        &store,
        P8_RECALL_MANIFEST_JSON,
        IN_MEMORY_LEXICAL_BASELINE_MODE,
    )
    .await
    .unwrap();

    assert_eq!(report.cases.len(), manifest.cases.len());
    assert_eq!(report.splits.len(), 2);
    assert!(report.overall.max_prompt_tokens <= 1_600);
    assert_eq!(
        report.manifest_sha256,
        RecallEvaluationManifest::sha256(P8_RECALL_MANIFEST_JSON)
    );
    serde_json::to_string_pretty(&report).unwrap();
    assert_p8_record_contracts_compile_through_store(&store).await;
}

#[tokio::test]
async fn gated_postgres_p8_1_lexical_baseline_is_replayable_from_clean_schema() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let manifest = RecallEvaluationManifest::parse(P8_RECALL_MANIFEST_JSON).unwrap();
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_recall_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    seed_p8_recall_fixture(&store, &manifest).await;

    let report = evaluate_lexical_recall_baseline(
        &store,
        P8_RECALL_MANIFEST_JSON,
        POSTGRES_LEXICAL_BASELINE_MODE,
    )
    .await
    .unwrap();
    assert_p8_record_contracts_compile_through_store(&store).await;
    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();

    if std::env::var("TM_UPDATE_P8_BASELINE").ok().as_deref() == Some("1") {
        let artifact = RecallBaselineArtifact {
            schema_version: 1,
            captured_at: report.measured_at,
            environment: RecallBaselineEnvironment {
                database: "PostgreSQL 16 clean isolated schema".to_string(),
                retrieval: "profile facts plus simple tsvector/plainto_tsquery with ILIKE fallback"
                    .to_string(),
                notes: "Seven warm query samples per case; setup and migrations excluded."
                    .to_string(),
            },
            report,
        };
        println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
        return;
    }

    let frozen: RecallBaselineArtifact = serde_json::from_str(P8_POSTGRES_BASELINE_JSON).unwrap();
    assert_eq!(report.deterministic(), frozen.report.deterministic());
    assert!(report.overall.max_prompt_tokens <= manifest.acceptance.max_prompt_tokens);
    for case in &report.cases {
        assert!(
            case.latency_p95_ms <= manifest.acceptance.latency_p95_ceiling_ms,
            "case {} p95 {}ms exceeded frozen {}ms ceiling",
            case.case_id,
            case.latency_p95_ms,
            manifest.acceptance.latency_p95_ceiling_ms
        );
    }
}
