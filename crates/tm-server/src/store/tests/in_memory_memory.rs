use super::*;

#[tokio::test]
async fn in_memory_p8_durable_records_filter_corrections_and_authority() {
    let store = InMemoryStore::default();
    let corrected_id = Uuid::new_v4();
    let replacement_id = Uuid::new_v4();
    let corrected = durable_episodic_record(
        corrected_id,
        "brian",
        "global",
        "The old passport date was July 1.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks::default(),
    );
    store.upsert_memory_record(corrected).await.unwrap();
    let replacement = durable_episodic_record(
        replacement_id,
        "brian",
        "global",
        "The passport date is August 20 after correction.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks {
            corrects_record_id: Some(corrected_id),
            ..MemoryRecordLinks::default()
        },
    );
    let replacement = store.upsert_memory_record(replacement).await.unwrap();
    let duplicate = store
        .upsert_memory_record(replacement.clone())
        .await
        .unwrap();
    assert_eq!(duplicate.content_key, replacement.content_key);
    let retried_with_new_id = store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "global",
            "The passport date is August 20 after correction.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    assert_eq!(retried_with_new_id.id(), replacement_id);
    store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "global",
            "An unsupported correction must remain withheld.",
            MemoryRecordStatus::Withheld,
            MemoryRecordLinks {
                corrects_record_id: Some(replacement_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();

    let scoped = durable_episodic_record(
        Uuid::new_v4(),
        "brian",
        "project:tempestmiku",
        "Project-only recall stays isolated.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks::default(),
    );
    store.upsert_memory_record(scoped.clone()).await.unwrap();
    let active = store
        .active_memory_records("brian", "global", 5)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id(), replacement_id);
    assert_eq!(
        store
            .active_memory_records("brian", "project:tempestmiku", 5)
            .await
            .unwrap()
            .iter()
            .map(StoredMemoryRecord::id)
            .collect::<Vec<_>>(),
        vec![scoped.id()]
    );
    assert!(matches!(
        store
            .memory_record(
                "alice",
                "global",
                MemoryRecordKind::Episodic,
                replacement_id
            )
            .await,
        Err(ServerError::NotFound(_))
    ));
    let cross_owner_link = durable_episodic_record(
        Uuid::new_v4(),
        "alice",
        "global",
        "Must not correct another owner record.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks {
            corrects_record_id: Some(replacement_id),
            ..MemoryRecordLinks::default()
        },
    );
    assert!(matches!(
        store.upsert_memory_record(cross_owner_link).await,
        Err(ServerError::NotFound(_))
    ));
}

#[tokio::test]
async fn in_memory_p8_withheld_successor_keeps_supported_record_retrievable() {
    let store = InMemoryStore::default();
    let supported_id = Uuid::new_v4();
    let withheld_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            supported_id,
            "brian",
            "global",
            "The supported release date remains July 1 pending review.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            withheld_id,
            "brian",
            "global",
            "An unreviewed correction is not owner truth.",
            MemoryRecordStatus::Withheld,
            MemoryRecordLinks {
                corrects_record_id: Some(supported_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            supported_id,
            "brian",
            "global",
            "The supported release date remains July 1 pending review.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks {
                corrected_by_record_id: Some(withheld_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .active_memory_records("brian", "global", 5)
            .await
            .unwrap()
            .iter()
            .map(StoredMemoryRecord::id)
            .collect::<Vec<_>>(),
        vec![supported_id]
    );
}

#[tokio::test]
async fn in_memory_p8_successor_filter_walks_inactive_intermediates_without_looping() {
    let store = InMemoryStore::default();
    let original_id = Uuid::new_v4();
    let intermediate_id = Uuid::new_v4();
    let final_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            original_id,
            "brian",
            "global",
            "Original value",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            intermediate_id,
            "brian",
            "global",
            "Withheld intermediate",
            MemoryRecordStatus::Withheld,
            MemoryRecordLinks {
                supersedes_record_id: Some(original_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();
    store
        .upsert_memory_record(durable_episodic_record(
            final_id,
            "brian",
            "global",
            "Final supported value",
            MemoryRecordStatus::Active,
            MemoryRecordLinks {
                supersedes_record_id: Some(intermediate_id),
                corrected_by_record_id: Some(intermediate_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();

    let active = store
        .active_memory_records("brian", "global", 5)
        .await
        .unwrap();
    assert_eq!(
        active
            .iter()
            .map(StoredMemoryRecord::id)
            .collect::<Vec<_>>(),
        vec![final_id]
    );
}

#[tokio::test]
async fn in_memory_p8_project_tombstone_cancels_future_embedding_access() {
    let store = InMemoryStore::default();
    let record = durable_episodic_record(
        Uuid::new_v4(),
        "brian",
        "project:revoked",
        "This project must disappear after unlink.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks::default(),
    );
    let record = store.upsert_memory_record(record).await.unwrap();
    let job = store
        .enqueue_memory_embedding_job(pending_embedding_job(&record))
        .await
        .unwrap();
    assert_eq!(job.status, tm_memory::MemoryEmbeddingJobStatus::Queued);

    let tombstone = store
        .revoke_memory_scope("brian", "project:revoked", "linked folder removed")
        .await
        .unwrap();
    assert_eq!(tombstone.memory_scope, "project:revoked");
    assert!(
        store
            .memory_scope_tombstone("brian", "project:revoked")
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        store
            .active_memory_records("brian", "project:revoked", 5)
            .await,
        Err(ServerError::NotFound(_))
    ));
    assert!(matches!(
        store
            .memory_embedding_jobs("brian", "project:revoked")
            .await,
        Err(ServerError::NotFound(_))
    ));
    assert!(matches!(
        store
            .enqueue_memory_embedding_job(pending_embedding_job(&record))
            .await,
        Err(ServerError::NotFound(_))
    ));
}
