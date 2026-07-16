use super::*;

#[tokio::test]
async fn gated_postgres_p8_durable_spine_migrates_filters_and_revokes() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let clean_schema = format!("tm_p8_clean_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {clean_schema}"))
        .await
        .unwrap();
    let clean = PostgresStore::connect_in_schema(&dsn, &clean_schema)
        .await
        .unwrap();
    assert_eq!(
        clean
            .memory_readiness(&EmbeddingConfig::default())
            .await
            .unwrap()
            .schema,
        MemorySchemaReadiness::Ready
    );
    assert!(
        clean
            .client()
            .query_one("select to_regclass('memory_records') is not null", &[])
            .await
            .unwrap()
            .get::<_, bool>(0)
    );
    drop(clean);
    admin
        .client()
        .batch_execute(&format!("drop schema {clean_schema} cascade"))
        .await
        .unwrap();

    let schema = format!("tm_p8_durable_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();

    let mut legacy_config = dsn.parse::<tokio_postgres::Config>().unwrap();
    legacy_config.options(format!("-c search_path={schema}"));
    let (legacy_client, legacy_connection) =
        legacy_config.connect(tokio_postgres::NoTls).await.unwrap();
    let legacy_connection_task = tokio::spawn(async move {
        legacy_connection.await.unwrap();
    });
    legacy_client
        .batch_execute(include_str!("../../../migrations/0001_base.sql"))
        .await
        .unwrap();
    let now = Utc::now();
    let fact_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let summary_id = Uuid::new_v4();
    legacy_client
        .execute(
            "insert into profile_facts(
                id, subject, predicate, object, confidence, importance, provenance, valid_from, valid_to
             ) values ($1, 'brian', 'prefers', 'durable evidence', 0.9, 0.8,
                       'memory://legacy/profile', $2, null)",
            &[&fact_id, &now],
        )
        .await
        .unwrap();
    legacy_client
        .execute(
            "insert into recall_chunks(id, scope, text, source, importance, created_at)
             values ($1, 'global', 'Passport date is August 20 and confirmed.',
                     'memory://legacy/recall', 0.9, $2)",
            &[&chunk_id, &now],
        )
        .await
        .unwrap();
    legacy_client
        .execute(
            "insert into memory_summaries(
                id, kind, subject, scope, title, body, evidence_json, source_dream_id,
                source_session_id, dedupe_key, created_at, updated_at
             ) values ($1, 'session', 'brian', 'global', 'Legacy summary',
                       'Keep lexical results stable during P8.2.', '[]'::jsonb, $2,
                       null, 'legacy-summary-p8', $3, $3)",
            &[&summary_id, &Uuid::new_v4(), &now],
        )
        .await
        .unwrap();
    drop(legacy_client);
    legacy_connection_task.await.unwrap();

    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let lexical_before = store.recall_chunks("global", "passport", 5).await.unwrap();
    assert_eq!(lexical_before.len(), 1);
    assert_eq!(lexical_before[0].id, chunk_id);
    assert_eq!(
        store
            .active_memory_records("owner", "global", 5)
            .await
            .unwrap()
            .iter()
            .map(StoredMemoryRecord::id)
            .collect::<Vec<_>>(),
        vec![chunk_id]
    );
    assert!(
        store
            .memory_record("brian", "global", MemoryRecordKind::Semantic, fact_id)
            .await
            .is_ok()
    );
    assert!(
        store
            .memory_record("brian", "global", MemoryRecordKind::Episodic, summary_id)
            .await
            .is_ok()
    );
    let evidence_count: i64 = store
        .client()
        .query_one("select count(*)::bigint from memory_record_evidence", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(evidence_count, 3);

    store.configure_owner_subject("brian").await.unwrap();
    let lexical_after = store.recall_chunks("global", "passport", 5).await.unwrap();
    assert_eq!(lexical_after, lexical_before);
    assert!(
        store
            .memory_record("brian", "global", MemoryRecordKind::Episodic, chunk_id)
            .await
            .is_ok()
    );

    let corrected_id = Uuid::new_v4();
    let replacement_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            corrected_id,
            "brian",
            "global",
            "The release date was July 1.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    let replacement = durable_episodic_record(
        replacement_id,
        "brian",
        "global",
        "The release date is August 20 after correction.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks {
            corrects_record_id: Some(corrected_id),
            ..MemoryRecordLinks::default()
        },
    );
    store
        .upsert_memory_record(replacement.clone())
        .await
        .unwrap();
    store.upsert_memory_record(replacement).await.unwrap();
    let retried_with_new_id = store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "global",
            "The release date is August 20 after correction.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    assert_eq!(retried_with_new_id.id(), replacement_id);
    let inactive_same_content_id = Uuid::new_v4();
    let reactivated_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            inactive_same_content_id,
            "brian",
            "global",
            "An inactive Postgres memory may be approved again.",
            MemoryRecordStatus::Withheld,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    let reactivated = store
        .upsert_memory_record(durable_episodic_record(
            reactivated_id,
            "brian",
            "global",
            "An inactive Postgres memory may be approved again.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    assert_eq!(reactivated.id(), reactivated_id);
    assert_eq!(
        store
            .memory_record(
                "brian",
                "global",
                MemoryRecordKind::Episodic,
                inactive_same_content_id,
            )
            .await
            .unwrap()
            .resource
            .status(),
        MemoryRecordStatus::Withheld
    );
    let active_ids = store
        .active_memory_records("brian", "global", 16)
        .await
        .unwrap()
        .into_iter()
        .map(|record| record.id())
        .collect::<Vec<_>>();
    assert!(active_ids.contains(&replacement_id));
    assert!(!active_ids.contains(&corrected_id));
    let supported_id = Uuid::new_v4();
    let withheld_id = Uuid::new_v4();
    store
        .upsert_memory_record(durable_episodic_record(
            supported_id,
            "brian",
            "global",
            "The supported status must survive an unreviewed correction.",
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
            "Candidate correction stays withheld until approved.",
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
            "The supported status must survive an unreviewed correction.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks {
                corrected_by_record_id: Some(withheld_id),
                ..MemoryRecordLinks::default()
            },
        ))
        .await
        .unwrap();
    assert!(
        store
            .active_memory_records("brian", "global", 16)
            .await
            .unwrap()
            .iter()
            .any(|record| record.id() == supported_id)
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
    let replacement_count: i64 = store
        .client()
        .query_one(
            "select count(*)::bigint from memory_records
              where record_kind = 'episodic' and id = $1",
            &[&replacement_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(replacement_count, 1);

    let project_record = store
        .upsert_memory_record(durable_episodic_record(
            Uuid::new_v4(),
            "brian",
            "project:unlinked",
            "The linked project must vanish immediately after unlink.",
            MemoryRecordStatus::Active,
            MemoryRecordLinks::default(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        store
            .memory_record(
                "brian",
                "global",
                MemoryRecordKind::Episodic,
                project_record.id()
            )
            .await,
        Err(ServerError::NotFound(_))
    ));
    let queued = store
        .enqueue_memory_embedding_job(pending_embedding_job(&project_record))
        .await
        .unwrap();
    let duplicate_job = store
        .enqueue_memory_embedding_job(pending_embedding_job(&project_record))
        .await
        .unwrap();
    assert_eq!(duplicate_job.id, queued.id);
    let alias = format!("p8-unlink-{}", Uuid::new_v4().simple());
    store
        .client()
        .execute(
            "insert into drive_links(
                alias, canonical_root, mode, linked_uri, memory_scope, project, status,
                metadata_json, created_at, updated_at, revoked_at, version, record_json
             ) values ($1, '/tmp/p8-unlink', 'ro', 'file:///tmp/p8-unlink',
                       'project:unlinked', 'unlinked', 'active', '{}'::jsonb,
                       now(), now(), null, 1, '{}'::jsonb)",
            &[&alias],
        )
        .await
        .unwrap();
    store
        .client()
        .execute(
            "update drive_links set status = 'revoked', revoked_at = now(), updated_at = now()
              where alias = $1",
            &[&alias],
        )
        .await
        .unwrap();
    assert!(
        store
            .memory_scope_tombstone("brian", "project:unlinked")
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        store
            .active_memory_records("brian", "project:unlinked", 5)
            .await,
        Err(ServerError::NotFound(_))
    ));
    let job_status: String = store
        .client()
        .query_one(
            "select status from memory_embedding_jobs where id = $1",
            &[&queued.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(job_status, "cancelled");
    let database_denial = store
        .client()
        .execute(
            "update memory_records set importance = importance
              where owner_subject = 'brian' and memory_scope = 'project:unlinked'",
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(
        database_denial
            .code()
            .map(tokio_postgres::error::SqlState::code),
        Some("TM001")
    );
    assert!(matches!(
        super::super::postgres::postgres_memory_error(database_denial),
        ServerError::NotFound(_)
    ));

    drop(store);
    let restarted = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    assert!(
        restarted
            .memory_scope_tombstone("brian", "project:unlinked")
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        restarted
            .active_memory_records("brian", "project:unlinked", 5)
            .await,
        Err(ServerError::NotFound(_))
    ));
    let restarted_job_status: String = restarted
        .client()
        .query_one(
            "select status from memory_embedding_jobs where id = $1",
            &[&queued.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(restarted_job_status, "cancelled");

    let readiness = restarted
        .memory_readiness(&EmbeddingConfig::default())
        .await
        .unwrap();
    assert_eq!(readiness.schema, MemorySchemaReadiness::Ready);
    assert!(readiness.allows_durable_writes());
    restarted
        .client()
        .batch_execute("drop table memory_record_evidence")
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .memory_readiness(&EmbeddingConfig::default())
            .await
            .unwrap()
            .schema,
        MemorySchemaReadiness::Corrupt { .. }
    ));

    drop(restarted);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
