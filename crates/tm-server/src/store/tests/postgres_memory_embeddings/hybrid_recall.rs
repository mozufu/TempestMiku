use super::super::*;

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
