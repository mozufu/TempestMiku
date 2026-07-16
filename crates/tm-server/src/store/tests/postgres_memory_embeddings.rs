use super::*;

#[tokio::test]
async fn gated_postgres_p8_hybrid_lexical_path_keeps_scope_and_corrections_when_dense_is_missing() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_hybrid_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    store.configure_owner_subject("brian").await.unwrap();

    let corrected_id = Uuid::new_v4();
    let replacement_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            corrected_id,
            "brian",
            "global",
            "The passport date was July 1.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            replacement_id,
            "brian",
            "global",
            "The passport date is August 20 after correction.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks {
                corrects_record_id: Some(corrected_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            replacement_id,
            "brian",
            "global",
            "The passport date is August 20 after correction.",
            MemoryRecordStatus::Withheld,
            MemoryRecordLinks {
                corrects_record_id: Some(corrected_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();
    let final_replacement_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            final_replacement_id,
            "brian",
            "global",
            "The passport date is September 1 after a second correction.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks {
                corrects_record_id: Some(replacement_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "project:other",
            "The passport date in another linked project must not leak.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();

    let request = tm_memory::HybridRecallRequest::default();
    let lexical = store
        .memory_lexical_candidates(&request, "passport date")
        .await
        .unwrap();
    assert_eq!(
        lexical
            .iter()
            .map(|candidate| candidate.record.id())
            .collect::<Vec<_>>(),
        vec![final_replacement_id]
    );
    let hybrid = store
        .memory_hybrid_candidates(&request, "passport date", None)
        .await
        .unwrap();
    assert_eq!(hybrid.candidates.len(), 1);
    assert_eq!(hybrid.candidates[0].record.id(), final_replacement_id);
    assert_eq!(hybrid.candidates[0].dense_rank, None);
    let tm_memory::MemoryRecordResource::Episodic(record) = &hybrid.candidates[0].record.resource
    else {
        panic!("fixture is episodic");
    };
    assert!(!record.evidence.is_empty());

    let config = EmbeddingConfig {
        provider: EmbeddingProvider::Local,
        dimensions: Some(3),
        model: Some("fixture-local-v1".to_string()),
        ..EmbeddingConfig::default()
    };
    let readiness = store.memory_readiness(&config).await.unwrap();
    let generation =
        NewMemoryEmbeddingGeneration::from_config("brian", "global", &config, Utc::now()).unwrap();
    if readiness.pgvector == PgVectorReadiness::Missing {
        assert!(matches!(
            store
                .stage_memory_embedding_generation(generation.clone())
                .await,
            Err(ServerError::Policy(_))
        ));
        assert!(
            store
                .active_memory_embedding_generation("brian", "global")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .memory_dense_candidates(
                    &request,
                    &DenseRecallQuery {
                        embedding_version: generation.embedding_version.clone(),
                        snapshot_revision: 0,
                        values: vec![1.0, 0.0, 0.0],
                    },
                )
                .await
                .unwrap()
                .is_none()
        );
    } else if readiness.pgvector == PgVectorReadiness::Ready {
        let first = store
            .stage_memory_embedding_generation(generation)
            .await
            .unwrap();
        assert_eq!(first.status, MemoryEmbeddingGenerationStatus::Staging);
        let worker_id = Uuid::new_v4();
        let first_lease = store
            .claim_memory_embedding_jobs(&embedding_claim(&first.embedding_version, worker_id))
            .await
            .unwrap();
        assert_eq!(first_lease.len(), 1);
        let completed = store
            .complete_memory_embedding_job(&first_lease[0], &[1.0, 0.0, 0.0], Utc::now())
            .await
            .unwrap();
        assert_eq!(completed.status, MemoryEmbeddingGenerationStatus::Ready);
        assert_eq!(
            store
                .active_memory_embedding_generation("brian", "global")
                .await
                .unwrap()
                .unwrap()
                .embedding_version,
            first.embedding_version
        );
        assert_eq!(
            store
                .memory_dense_candidates(&request, &dense_query(&first, vec![1.0, 0.0, 0.0]))
                .await
                .unwrap()
                .unwrap()
                .iter()
                .map(|candidate| candidate.record.id())
                .collect::<Vec<_>>(),
            vec![final_replacement_id]
        );

        let second_config = EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            dimensions: Some(3),
            model: Some("fixture-local-v2".to_string()),
            ..EmbeddingConfig::default()
        };
        let second = store
            .stage_memory_embedding_generation(
                NewMemoryEmbeddingGeneration::from_config(
                    "brian",
                    "global",
                    &second_config,
                    Utc::now(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status, MemoryEmbeddingGenerationStatus::Staging);
        assert_eq!(
            store
                .active_memory_embedding_generation("brian", "global")
                .await
                .unwrap()
                .unwrap()
                .embedding_version,
            first.embedding_version
        );
        assert!(matches!(
            crate::memory::run_memory_embedding_batch(
                &store,
                &ProviderDown,
                &second,
                worker_id,
                Utc::now(),
                Duration::seconds(30),
                second_config.clone(),
            )
            .await,
            Err(ServerError::Store(_))
        ));
        assert!(
            store
                .memory_embedding_jobs("brian", "global")
                .await
                .unwrap()
                .iter()
                .any(|job| {
                    job.provenance.embedding_version == second.embedding_version
                        && job.status == tm_memory::MemoryEmbeddingJobStatus::Queued
                })
        );
        assert_eq!(
            store
                .active_memory_embedding_generation("brian", "global")
                .await
                .unwrap()
                .unwrap()
                .embedding_version,
            first.embedding_version
        );
        let second_lease = store
            .claim_memory_embedding_jobs(&embedding_claim(&second.embedding_version, worker_id))
            .await
            .unwrap();
        assert_eq!(second_lease.len(), 1);
        let second_completed = store
            .complete_memory_embedding_job(&second_lease[0], &[1.0, 0.0, 0.0], Utc::now())
            .await
            .unwrap();
        assert_eq!(
            second_completed.status,
            MemoryEmbeddingGenerationStatus::Ready
        );
        assert_eq!(
            store
                .active_memory_embedding_generation("brian", "global")
                .await
                .unwrap()
                .unwrap()
                .embedding_version,
            second.embedding_version
        );
        assert!(matches!(
            store
                .memory_dense_candidates(&request, &dense_query(&second, vec![1.0, 0.0]))
                .await,
            Err(ServerError::InvalidRequest(_))
        ));
        assert_eq!(
            store
                .memory_hybrid_candidates(
                    &request,
                    "passport date",
                    Some(&dense_query(&second, vec![1.0, 0.0])),
                )
                .await
                .unwrap()
                .candidates
                .iter()
                .map(|candidate| candidate.record.id())
                .collect::<Vec<_>>(),
            vec![final_replacement_id]
        );

        let third_config = EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            dimensions: Some(3),
            model: Some("fixture-local-v3".to_string()),
            ..EmbeddingConfig::default()
        };
        let third = store
            .stage_memory_embedding_generation(
                NewMemoryEmbeddingGeneration::from_config(
                    "brian",
                    "global",
                    &third_config,
                    Utc::now(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let stale_lease = store
            .claim_memory_embedding_jobs(&embedding_claim(&third.embedding_version, worker_id))
            .await
            .unwrap();
        assert_eq!(stale_lease.len(), 1);
        let stale_locked_at = stale_lease[0].job.locked_at.unwrap();
        let replacement_worker_id = Uuid::new_v4();
        let reclaimed_lease = store
            .claim_memory_embedding_jobs(&MemoryEmbeddingJobClaim {
                owner_subject: "brian".to_string(),
                memory_scope: "global".to_string(),
                embedding_version: third.embedding_version.clone(),
                owner_id: replacement_worker_id,
                now: stale_locked_at + Duration::seconds(31),
                lease_timeout: Duration::seconds(30),
                limit: 3,
            })
            .await
            .unwrap();
        assert_eq!(reclaimed_lease.len(), 1);
        assert!(matches!(
            store
                .complete_memory_embedding_job(
                    &stale_lease[0],
                    &[1.0, 0.0, 0.0],
                    stale_locked_at + Duration::seconds(32),
                )
                .await,
            Err(ServerError::NotFound(_))
        ));
        let stale_vector_count: i64 = store
            .client()
            .query_one(
                &format!(
                    "select count(*) from {} where record_id = $1",
                    third.vector_table
                ),
                &[&stale_lease[0].job.record_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(stale_vector_count, 0);
        store
            .upsert_memory_record(durable_episodic_record(
                final_replacement_id,
                "brian",
                "global",
                "The passport date is October 1 after a final correction.",
                MemoryRecordStatus::Active,
                MemoryRecordLinks {
                    corrects_record_id: Some(replacement_id),
                    ..MemoryRecordLinks::default()
                },
            ))
            .await
            .unwrap();
        assert!(matches!(
            store
                .complete_memory_embedding_job(
                    &reclaimed_lease[0],
                    &[1.0, 0.0, 0.0],
                    stale_locked_at + Duration::seconds(33),
                )
                .await,
            Err(ServerError::Conflict(_))
        ));
        assert_eq!(
            store
                .memory_embedding_generation("brian", "global", &third.embedding_version)
                .await
                .unwrap()
                .status,
            MemoryEmbeddingGenerationStatus::Failed
        );
        assert!(
            store
                .memory_dense_candidates(&request, &dense_query(&third, vec![1.0, 0.0, 0.0]))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .memory_hybrid_candidates(
                    &request,
                    "passport date",
                    Some(&dense_query(&third, vec![1.0, 0.0, 0.0])),
                )
                .await
                .unwrap()
                .candidates
                .iter()
                .map(|candidate| candidate.record.id())
                .collect::<Vec<_>>(),
            vec![final_replacement_id]
        );
    }

    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}

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

#[tokio::test]
async fn gated_postgres_p8_durable_migration_failure_rolls_back_only_v14() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_rollback_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let mut config = dsn.parse::<tokio_postgres::Config>().unwrap();
    config.options(format!("-c search_path={schema}"));
    let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
    let connection_task = tokio::spawn(async move {
        connection.await.unwrap();
    });
    // The malformed pre-existing relation is accepted by `create table if not exists` but makes
    // the later P8.2 evidence FK invalid. The ordered migration runner must leave version 14
    // unapplied and roll back every relation it created in that migration.
    client
        .batch_execute("create table memory_records(id uuid primary key)")
        .await
        .unwrap();
    drop(client);
    connection_task.await.unwrap();

    assert!(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .is_err()
    );

    let mut inspect_config = dsn.parse::<tokio_postgres::Config>().unwrap();
    inspect_config.options(format!("-c search_path={schema}"));
    let (inspect, inspect_connection) =
        inspect_config.connect(tokio_postgres::NoTls).await.unwrap();
    let inspect_connection_task = tokio::spawn(async move {
        inspect_connection.await.unwrap();
    });
    let max_version: i64 = inspect
        .query_one("select max(version)::bigint from schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(max_version, 13);
    let evidence_table_exists: bool = inspect
        .query_one(
            "select to_regclass('memory_record_evidence') is not null",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!evidence_table_exists);
    drop(inspect);
    inspect_connection_task.await.unwrap();
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
