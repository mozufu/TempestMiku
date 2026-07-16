use super::*;

#[tokio::test]
async fn gated_postgres_p8_hybrid_provider_improves_quality_and_survives_restart_and_loss() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let manifest = RecallEvaluationManifest::parse(P8_RECALL_MANIFEST_JSON).unwrap();
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_provider_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = Arc::new(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .unwrap(),
    );
    store.configure_owner_subject("brian").await.unwrap();
    seed_p8_recall_fixture(store.as_ref(), &manifest).await;
    seed_p8_hybrid_fixture(store.as_ref(), &manifest).await;

    let config = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(3),
        model: Some("fixture-local-quality-v1".to_string()),
        ..EmbeddingConfig::default()
    };
    let client: Arc<dyn tm_memory::EmbeddingClient> = Arc::new(ConstantEmbeddingClient);
    for scope in store.active_memory_scopes("brian").await.unwrap() {
        let generation = store
            .stage_memory_embedding_generation(
                NewMemoryEmbeddingGeneration::from_config("brian", &scope, &config, Utc::now())
                    .unwrap(),
            )
            .await
            .unwrap();
        run_memory_embedding_batch(
            store.as_ref(),
            client.as_ref(),
            &generation,
            Uuid::new_v4(),
            Utc::now(),
            chrono::Duration::seconds(30),
            config.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .active_memory_embedding_generation("brian", &scope)
                .await
                .unwrap()
                .unwrap()
                .embedding_version,
            generation.embedding_version
        );
    }

    let provider = StoreMemoryProvider::new(Arc::clone(&store))
        .with_embeddings(config.clone(), Arc::clone(&client))
        .unwrap();
    let report = evaluate_memory_provider_recall(
        &provider,
        P8_RECALL_MANIFEST_JSON,
        POSTGRES_HYBRID_RECALL_MODE,
    )
    .await
    .unwrap();
    let baseline: RecallBaselineArtifact = serde_json::from_str(P8_POSTGRES_BASELINE_JSON).unwrap();
    assert!(
        report.overall.mean_ndcg_at_5
            >= baseline.report.overall.mean_ndcg_at_5
                * (1.0 + manifest.acceptance.min_relative_ndcg_at_5_improvement)
    );
    assert!(report.overall.mean_recall_at_5 >= baseline.report.overall.mean_recall_at_5);
    let hybrid_held = report
        .splits
        .iter()
        .find(|split| split.split == tm_memory::RecallEvaluationSplit::HeldOut)
        .unwrap();
    let baseline_held = baseline
        .report
        .splits
        .iter()
        .find(|split| split.split == tm_memory::RecallEvaluationSplit::HeldOut)
        .unwrap();
    assert!(
        hybrid_held.metrics.mean_ndcg_at_5
            >= baseline_held.metrics.mean_ndcg_at_5
                * (1.0 + manifest.acceptance.min_relative_ndcg_at_5_improvement)
    );
    assert!(hybrid_held.metrics.mean_recall_at_5 >= baseline_held.metrics.mean_recall_at_5);
    assert_eq!(
        report.overall.false_inclusions,
        tm_memory::RecallFalseInclusionCounts::default()
    );
    assert!(report.cases.iter().all(|case| case.candidate_count
        <= manifest.acceptance.max_final_recall_items
        && case.prompt_tokens <= manifest.acceptance.max_prompt_tokens));

    let first = provider
        .context_for_turn("brian", "global", "appointment passport date")
        .await
        .unwrap();
    assert_eq!(first.retrieval.mode, MemoryRetrievalMode::Hybrid);
    let first_ids = first
        .hybrid_recall
        .iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    drop(provider);
    drop(store);

    let restarted = Arc::new(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .unwrap(),
    );
    let restarted_provider = StoreMemoryProvider::new(Arc::clone(&restarted))
        .with_embeddings(config.clone(), Arc::clone(&client))
        .unwrap();
    let after_restart = restarted_provider
        .context_for_turn("brian", "global", "appointment passport date")
        .await
        .unwrap();
    assert_eq!(
        after_restart
            .hybrid_recall
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        first_ids
    );
    let summary_session = restarted
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let summary = restarted
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            title: "Unique summary mirror marker".to_string(),
            body: "The unique summary mirror marker must appear only once.".to_string(),
            evidence: Vec::new(),
            source_dream_id: Uuid::new_v4(),
            source_session_id: Some(summary_session.id),
            dedupe_key: format!("summary:mirror-filter:{}", summary_session.id),
        })
        .await
        .unwrap();
    let summary_provider = StoreMemoryProvider::new(Arc::clone(&restarted))
        .with_recall_limit(10)
        .with_summary_limit(10)
        .with_prompt_budget_tokens(10_000)
        .with_embeddings(config.clone(), Arc::clone(&client))
        .unwrap();
    let summary_context = summary_provider
        .context_for_turn("brian", "global", "unique summary mirror marker")
        .await
        .unwrap();
    assert_eq!(
        summary_context
            .summaries
            .iter()
            .filter(|item| item.id == summary.id)
            .count(),
        1
    );
    assert!(
        summary_context
            .hybrid_recall
            .iter()
            .all(|item| item.id != summary.id),
        "the mirrored typed summary must not consume a second recall slot"
    );
    let unavailable: Arc<dyn tm_memory::EmbeddingClient> = Arc::new(UnavailableEmbeddingClient);
    let degraded = StoreMemoryProvider::new(Arc::clone(&restarted))
        .with_embeddings(config, unavailable)
        .unwrap()
        .context_for_turn("brian", "global", "appointment passport date")
        .await
        .unwrap();
    assert_eq!(
        degraded.retrieval.mode,
        MemoryRetrievalMode::LexicalFallback
    );
    assert_eq!(
        degraded.retrieval.degraded_reason.as_deref(),
        Some("no_active_embedding_generation")
    );
    assert_eq!(
        degraded.hybrid_recall.first().map(|item| item.id),
        Some(Uuid::parse_str("20000000-0000-0000-0000-000000000102").unwrap())
    );

    drop(restarted_provider);
    drop(restarted);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

#[tokio::test]
async fn gated_live_lumo_embedding_replays_frozen_quality_fixture() {
    let Ok(endpoint) = std::env::var("TM_P8_LIVE_EMBEDDING_ENDPOINT") else {
        return;
    };
    let Some(dsn) = postgres_test_dsn() else {
        panic!("TM_P8_LIVE_EMBEDDING_ENDPOINT requires the gated Postgres test environment");
    };
    let model = std::env::var("TM_P8_LIVE_EMBEDDING_MODEL")
        .expect("live P8 evidence pins TM_P8_LIVE_EMBEDDING_MODEL");
    let dimensions = std::env::var("TM_P8_LIVE_EMBEDDING_DIMENSIONS")
        .expect("live P8 evidence pins TM_P8_LIVE_EMBEDDING_DIMENSIONS")
        .parse::<usize>()
        .expect("live embedding dimensions must be an integer");
    let manifest = RecallEvaluationManifest::parse(P8_RECALL_MANIFEST_JSON).unwrap();
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let configured_schema = std::env::var("TM_P8_LIVE_SCHEMA").ok();
    let schema = configured_schema
        .clone()
        .unwrap_or_else(|| format!("tm_p8_lumo_{}", Uuid::new_v4().simple()));
    assert!(
        schema
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        "live schema must be a safe unquoted Postgres identifier"
    );
    if configured_schema.is_some() {
        admin
            .client()
            .batch_execute(&format!("drop schema if exists {schema} cascade"))
            .await
            .unwrap();
    }
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = Arc::new(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .unwrap(),
    );
    store.configure_owner_subject("brian").await.unwrap();
    seed_p8_recall_fixture(store.as_ref(), &manifest).await;
    seed_p8_hybrid_fixture(store.as_ref(), &manifest).await;

    let config = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(dimensions),
        model: Some(model.clone()),
        timeout_ms: 120_000,
        max_batch_size: 16,
        ..EmbeddingConfig::default()
    };
    let client: Arc<dyn EmbeddingClient> =
        Arc::new(LocalEmbeddingHttpClient::new(reqwest::Url::parse(&endpoint).unwrap()).unwrap());
    let probe = EmbeddingRequest::new(
        config.clone(),
        vec![EmbeddingInput::new(
            "lumo-probe",
            "search_query: appointment passport date",
        )],
    )
    .unwrap();
    let cold_started = Instant::now();
    let cold = client.embed(probe.clone()).await.unwrap();
    let cold_latency_ms = cold_started.elapsed().as_millis();
    cold.validate_for(&probe).unwrap();
    let warm_started = Instant::now();
    let warm = client.embed(probe.clone()).await.unwrap();
    let warm_latency_ms = warm_started.elapsed().as_millis();
    warm.validate_for(&probe).unwrap();
    assert_eq!(cold.vectors, warm.vectors);

    let embedding_started = Instant::now();
    let mut embedded_records = 0;
    for scope in store.active_memory_scopes("brian").await.unwrap() {
        let generation = store
            .stage_memory_embedding_generation(
                NewMemoryEmbeddingGeneration::from_config("brian", &scope, &config, Utc::now())
                    .unwrap(),
            )
            .await
            .unwrap();
        loop {
            let completed = run_memory_embedding_batch(
                store.as_ref(),
                client.as_ref(),
                &generation,
                Uuid::new_v4(),
                Utc::now(),
                chrono::Duration::seconds(150),
                config.clone(),
            )
            .await
            .unwrap();
            embedded_records += completed;
            if completed == 0 {
                break;
            }
        }
        assert!(
            store
                .active_memory_embedding_generation("brian", &scope)
                .await
                .unwrap()
                .is_some()
        );
    }
    let embedding_elapsed_ms = embedding_started.elapsed().as_millis();

    let provider = StoreMemoryProvider::new(Arc::clone(&store))
        .with_embeddings(config.clone(), Arc::clone(&client))
        .unwrap();
    let report = evaluate_memory_provider_recall(
        &provider,
        P8_RECALL_MANIFEST_JSON,
        POSTGRES_HYBRID_RECALL_MODE,
    )
    .await
    .unwrap();
    let baseline: RecallBaselineArtifact = serde_json::from_str(P8_POSTGRES_BASELINE_JSON).unwrap();
    assert!(
        report.overall.mean_ndcg_at_5
            >= baseline.report.overall.mean_ndcg_at_5
                * (1.0 + manifest.acceptance.min_relative_ndcg_at_5_improvement)
    );
    assert!(report.overall.mean_recall_at_5 >= baseline.report.overall.mean_recall_at_5);
    assert_eq!(
        report.overall.false_inclusions,
        tm_memory::RecallFalseInclusionCounts::default()
    );
    const LIVE_LOCAL_P95_CEILING_MS: f64 = 5_000.0;
    assert!(
        report.overall.latency_p95_ms <= LIVE_LOCAL_P95_CEILING_MS,
        "live local provider p95 {}ms exceeded {}ms",
        report.overall.latency_p95_ms,
        LIVE_LOCAL_P95_CEILING_MS
    );
    let reembedding_scope = "project:p8-reembedding";
    let reembedding_id = Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap();
    store
        .upsert_memory_record(
            StoredMemoryRecord::new(MemoryRecordResource::Episodic(EpisodicMemoryRecord {
                schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
                id: reembedding_id,
                owner_subject: "brian".to_string(),
                memory_scope: reembedding_scope.to_string(),
                text: "Resumable local embedding evidence survives an expired worker lease."
                    .to_string(),
                evidence: vec![MemoryRecordEvidence::resource(
                    "memory://evidence/p8-lumo-reembedding",
                    "P8.5 lumo canary",
                )],
                confidence: 1.0,
                importance: 0.8,
                observed_at: Utc::now(),
                effective_from: Utc::now(),
                effective_to: None,
                status: MemoryRecordStatus::Active,
                links: MemoryRecordLinks::default(),
                created_at: Utc::now(),
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let reembedding_generation = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config(
                "brian",
                reembedding_scope,
                &config,
                Utc::now(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let claimed_at = Utc::now();
    let abandoned = store
        .claim_memory_embedding_jobs(&MemoryEmbeddingJobClaim {
            owner_subject: "brian".to_string(),
            memory_scope: reembedding_scope.to_string(),
            embedding_version: reembedding_generation.embedding_version.clone(),
            owner_id: Uuid::new_v4(),
            now: claimed_at,
            lease_timeout: chrono::Duration::seconds(1),
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(abandoned.len(), 1);
    let reembedded_records = run_memory_embedding_batch(
        store.as_ref(),
        client.as_ref(),
        &reembedding_generation,
        Uuid::new_v4(),
        claimed_at + chrono::Duration::seconds(2),
        chrono::Duration::seconds(1),
        config.clone(),
    )
    .await
    .unwrap();
    assert_eq!(reembedded_records, 1);
    let reembedding_generation = store
        .active_memory_embedding_generation("brian", reembedding_scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reembedding_generation.completed_records, 1);
    let unavailable: Arc<dyn EmbeddingClient> = Arc::new(
        LocalEmbeddingHttpClient::new(
            reqwest::Url::parse("http://127.0.0.1:9/v1/embeddings").unwrap(),
        )
        .unwrap(),
    );
    let degraded = StoreMemoryProvider::new(Arc::clone(&store))
        .with_embeddings(config.clone(), unavailable)
        .unwrap()
        .context_for_turn("brian", "global", "appointment passport date")
        .await
        .unwrap();
    assert_eq!(
        degraded.retrieval.mode,
        MemoryRetrievalMode::LexicalFallback
    );
    assert_eq!(
        degraded.retrieval.degraded_reason.as_deref(),
        Some("embedding_provider_unavailable")
    );
    println!(
        "P8_LUMO_EVIDENCE={}",
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 1,
            "host": "lumo",
            "provider": "local",
            "model": model,
            "modelDigest": std::env::var("TM_P8_LIVE_EMBEDDING_MODEL_DIGEST").ok(),
            "dimensions": dimensions,
            "embeddingVersion": config.embedding_version().unwrap(),
            "coldLatencyMs": cold_latency_ms,
            "warmLatencyMs": warm_latency_ms,
            "embeddedRecords": embedded_records,
            "embeddingElapsedMs": embedding_elapsed_ms,
            "liveLocalP95CeilingMs": LIVE_LOCAL_P95_CEILING_MS,
            "resumableReembedding": {
                "scope": reembedding_scope,
                "recordId": reembedding_id,
                "abandonedLeaseCount": abandoned.len(),
                "reembeddedRecords": reembedded_records,
                "generationStatus": reembedding_generation.status,
                "completedRecords": reembedding_generation.completed_records,
            },
            "providerLoss": {
                "mode": degraded.retrieval.mode,
                "reason": degraded.retrieval.degraded_reason,
                "candidateCount": degraded.hybrid_recall.len(),
            },
            "report": report,
        }))
        .unwrap()
    );

    drop(provider);
    drop(store);
    if configured_schema.is_none() {
        admin
            .client()
            .batch_execute(&format!("drop schema {schema} cascade"))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn gated_live_lumo_server_restart_reuses_persisted_generation() {
    let Ok(schema) = std::env::var("TM_P8_LIVE_RESTART_SCHEMA") else {
        return;
    };
    let Some(dsn) = postgres_test_dsn() else {
        panic!("TM_P8_LIVE_RESTART_SCHEMA requires the gated Postgres test environment");
    };
    assert!(
        schema
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        "live schema must be a safe unquoted Postgres identifier"
    );
    let endpoint = std::env::var("TM_P8_LIVE_EMBEDDING_ENDPOINT").unwrap();
    let model = std::env::var("TM_P8_LIVE_EMBEDDING_MODEL").unwrap();
    let dimensions = std::env::var("TM_P8_LIVE_EMBEDDING_DIMENSIONS")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let config = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(dimensions),
        model: Some(model),
        timeout_ms: 120_000,
        max_batch_size: 16,
        ..EmbeddingConfig::default()
    };
    let client: Arc<dyn EmbeddingClient> =
        Arc::new(LocalEmbeddingHttpClient::new(reqwest::Url::parse(&endpoint).unwrap()).unwrap());
    let store = Arc::new(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .unwrap(),
    );
    let provider = StoreMemoryProvider::new(Arc::clone(&store))
        .with_embeddings(config, client)
        .unwrap();
    let report = evaluate_memory_provider_recall(
        &provider,
        P8_RECALL_MANIFEST_JSON,
        POSTGRES_HYBRID_RECALL_MODE,
    )
    .await
    .unwrap();
    let manifest = RecallEvaluationManifest::parse(P8_RECALL_MANIFEST_JSON).unwrap();
    let baseline: RecallBaselineArtifact = serde_json::from_str(P8_POSTGRES_BASELINE_JSON).unwrap();
    assert!(
        report.overall.mean_ndcg_at_5
            >= baseline.report.overall.mean_ndcg_at_5
                * (1.0 + manifest.acceptance.min_relative_ndcg_at_5_improvement)
    );
    assert!(report.overall.mean_recall_at_5 >= baseline.report.overall.mean_recall_at_5);
    assert_eq!(
        report.overall.false_inclusions,
        tm_memory::RecallFalseInclusionCounts::default()
    );
    println!(
        "P8_LUMO_RESTART_EVIDENCE={}",
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 1,
            "host": "lumo",
            "reusedSchema": schema,
            "retrievalMode": report.retrieval_mode,
            "overall": report.overall,
            "splits": report.splits,
        }))
        .unwrap()
    );

    drop(provider);
    drop(store);
    if std::env::var("TM_P8_LIVE_CLEANUP").ok().as_deref() == Some("1") {
        let admin = PostgresStore::connect(&dsn).await.unwrap();
        admin
            .client()
            .batch_execute(&format!("drop schema {schema} cascade"))
            .await
            .unwrap();
    }
}
