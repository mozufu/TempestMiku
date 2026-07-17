use super::super::*;

#[tokio::test]
async fn gated_postgres_p8_active_generation_incrementally_embeds_new_records() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_incremental_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    store.configure_owner_subject("brian").await.unwrap();
    let readiness = store
        .memory_readiness(&EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            dimensions: Some(3),
            model: Some("fixture-incremental-v1".to_string()),
            ..EmbeddingConfig::default()
        })
        .await
        .unwrap();
    if readiness.pgvector != PgVectorReadiness::Ready {
        drop(store);
        admin
            .client()
            .batch_execute(&format!("drop schema {schema} cascade"))
            .await
            .unwrap();
        return;
    }

    let first_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            first_id,
            "brian",
            "global",
            "first incrementally embedded memory",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    let config = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(3),
        model: Some("fixture-incremental-v1".to_string()),
        ..EmbeddingConfig::default()
    };
    let generation = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &config, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let worker_id = Uuid::new_v4();
    let first_lease = store
        .claim_memory_embedding_jobs(&embedding_claim(&generation.embedding_version, worker_id))
        .await
        .unwrap();
    assert_eq!(first_lease.len(), 1);
    let completed = store
        .complete_memory_embedding_job(&first_lease[0], &[1.0, 0.0, 0.0], Utc::now())
        .await
        .unwrap();
    let pointer_before: chrono::DateTime<Utc> = store
        .client()
        .query_one(
            "select activated_at from memory_embedding_active_versions
              where owner_subject = 'brian' and memory_scope = 'global'",
            &[],
        )
        .await
        .unwrap()
        .get("activated_at");
    let jobs_before = store
        .memory_embedding_jobs("brian", "global")
        .await
        .unwrap()
        .len();
    let unchanged = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &config, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let pointer_after: chrono::DateTime<Utc> = store
        .client()
        .query_one(
            "select activated_at from memory_embedding_active_versions
              where owner_subject = 'brian' and memory_scope = 'global'",
            &[],
        )
        .await
        .unwrap()
        .get("activated_at");
    assert_eq!(unchanged.updated_at, completed.updated_at);
    assert_eq!(pointer_after, pointer_before);
    assert_eq!(
        store
            .memory_embedding_jobs("brian", "global")
            .await
            .unwrap()
            .len(),
        jobs_before
    );

    let second_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            second_id,
            "brian",
            "global",
            "second memory added after active pointer",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    let refreshed = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &config, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.status, MemoryEmbeddingGenerationStatus::Staging);
    assert_eq!(refreshed.expected_records, 2);
    assert!(
        store
            .active_memory_embedding_generation("brian", "global")
            .await
            .unwrap()
            .is_none(),
        "a dirty snapshot must not be served while incremental coverage is partial"
    );
    assert!(
        store
            .memory_dense_candidates(
                &HybridRecallRequest::default(),
                &dense_query(&refreshed, vec![1.0, 0.0, 0.0]),
            )
            .await
            .unwrap()
            .is_none()
    );
    let second_lease = store
        .claim_memory_embedding_jobs(&embedding_claim(&generation.embedding_version, worker_id))
        .await
        .unwrap();
    assert_eq!(second_lease.len(), 1);
    assert_eq!(second_lease[0].job.record_id, second_id);
    let completed = store
        .complete_memory_embedding_job(&second_lease[0], &[1.0, 0.0, 0.0], Utc::now())
        .await
        .unwrap();
    assert_eq!(completed.status, MemoryEmbeddingGenerationStatus::Ready);
    assert_eq!(completed.completed_records, 2);
    assert_eq!(
        store
            .active_memory_embedding_generation("brian", "global")
            .await
            .unwrap()
            .unwrap()
            .embedding_version,
        generation.embedding_version
    );
    let dense = store
        .memory_dense_candidates(
            &HybridRecallRequest::default(),
            &dense_query(&completed, vec![1.0, 0.0, 0.0]),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dense.len(), 2);
    assert!(
        dense
            .iter()
            .any(|candidate| candidate.record.id() == first_id)
    );
    assert!(
        dense
            .iter()
            .any(|candidate| candidate.record.id() == second_id)
    );

    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

#[tokio::test]
async fn gated_postgres_p8_embedding_batch_isolates_oversized_input_and_recovers_on_raise() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_batch_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    store.configure_owner_subject("brian").await.unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "global",
            "short",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "global",
            &"oversized ".repeat(40),
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    let small = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(3),
        model: Some("fixture-batch-isolation".to_string()),
        max_input_bytes: 64,
        ..EmbeddingConfig::default()
    };
    let generation = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &small, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    let completed = crate::memory::run_memory_embedding_batch(
        &store,
        &UnitEmbeddingClient,
        &generation,
        Uuid::new_v4(),
        Utc::now(),
        Duration::seconds(30),
        small.clone(),
    )
    .await
    .unwrap();
    assert_eq!(completed, 1);
    let jobs = store
        .memory_embedding_jobs("brian", "global")
        .await
        .unwrap();
    assert_eq!(
        jobs.iter()
            .filter(|job| job.status == tm_memory::MemoryEmbeddingJobStatus::Completed)
            .count(),
        1
    );
    assert_eq!(
        jobs.iter()
            .filter(|job| job.failure_code.as_deref() == Some("input_too_large"))
            .count(),
        1
    );
    let failed = store
        .memory_embedding_generation("brian", "global", &generation.embedding_version)
        .await
        .unwrap();
    assert_eq!(failed.status, MemoryEmbeddingGenerationStatus::Failed);
    let unchanged = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &small, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unchanged.updated_at, failed.updated_at);

    let raised = EmbeddingConfig {
        max_input_bytes: 512,
        ..small
    };
    let recovering = store
        .stage_memory_embedding_generation(
            NewMemoryEmbeddingGeneration::from_config("brian", "global", &raised, Utc::now())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovering.status, MemoryEmbeddingGenerationStatus::Staging);
    assert_eq!(
        crate::memory::run_memory_embedding_batch(
            &store,
            &UnitEmbeddingClient,
            &recovering,
            Uuid::new_v4(),
            Utc::now(),
            Duration::seconds(30),
            raised,
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        store
            .active_memory_embedding_generation("brian", "global")
            .await
            .unwrap()
            .unwrap()
            .completed_records,
        2
    );

    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
