use super::super::*;

#[tokio::test]
async fn gated_postgres_p8_missing_vector_table_reports_dense_fallback() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_dense_fallback_{}", Uuid::new_v4().simple());
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
    let config = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(3),
        model: Some("fixture-dense-fallback-v1".to_string()),
        ..EmbeddingConfig::default()
    };
    if store.memory_readiness(&config).await.unwrap().pgvector != PgVectorReadiness::Ready {
        drop(store);
        admin
            .client()
            .batch_execute(&format!("drop schema {schema} cascade"))
            .await
            .unwrap();
        return;
    }

    let record_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            record_id,
            "brian",
            "global",
            "Dense fallback must remain visible in the recall trace.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    let generation = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &config, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let lease = store
        .claim_memory_embedding_jobs(&embedding_claim(
            &generation.embedding_version,
            Uuid::new_v4(),
        ))
        .await
        .unwrap();
    let ready = store
        .complete_memory_embedding_job(&lease[0], &[1.0, 0.0, 0.0], Utc::now())
        .await
        .unwrap();
    store
        .client()
        .batch_execute(&format!("drop table {}", ready.vector_table))
        .await
        .unwrap();

    let request = HybridRecallRequest::default();
    let hybrid = store
        .memory_hybrid_candidates(
            &request,
            "dense fallback",
            Some(&dense_query(&ready, vec![1.0, 0.0, 0.0])),
        )
        .await
        .unwrap();
    assert_eq!(hybrid.dense_status, DenseRecallStatus::Unavailable);
    assert_eq!(hybrid.candidates[0].record.id(), record_id);

    let embedding_client: Arc<dyn tm_memory::EmbeddingClient> = Arc::new(UnitEmbeddingClient);
    let provider = crate::StoreMemoryProvider::new(Arc::clone(&store))
        .with_embeddings(config, embedding_client)
        .unwrap();
    let context =
        crate::MemoryProvider::context_for_turn(&provider, "brian", "global", "dense fallback")
            .await
            .unwrap();
    assert_eq!(
        context.retrieval.mode,
        crate::MemoryRetrievalMode::LexicalFallback
    );
    assert_eq!(
        context.retrieval.degraded_reason.as_deref(),
        Some("dense_backend_unavailable")
    );

    drop(provider);
    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
