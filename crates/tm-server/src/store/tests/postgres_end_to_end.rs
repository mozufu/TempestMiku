use super::*;

#[tokio::test]
async fn gated_postgres_covers_replay_memory_approvals_and_project_refs() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();

    let ended = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await
        .unwrap();
    assert_eq!(ended.session.status, "ended");
    assert!(ended.newly_ended);
    assert_eq!(
        ended
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["dream_queued", "session_end"]
    );
    assert_eq!(ended.dream.source_event_seq, Some(ended.events[1].seq));
    assert!(ended.events[0].seq < ended.events[1].seq);
    let dream = ended.dream;
    let duplicate = store
        .end_session_and_enqueue_dream(session.id, "brian".to_string(), "global".to_string())
        .await
        .unwrap();
    assert!(!duplicate.newly_ended);
    assert!(duplicate.events.is_empty());
    assert_eq!(duplicate.dream.id, dream.id);
    assert_eq!(duplicate.dream.status, DreamStatus::Queued);
    assert_eq!(
        store
            .dream_queue_for_session(session.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let session_end_claim = store
        .claim_ready_dream(Utc::now(), Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres session-end dream");
    assert_eq!(session_end_claim.dream.id, dream.id);
    let completed_session_end = store
        .complete_dream(&session_end_claim, Utc::now())
        .await
        .unwrap();
    assert_eq!(completed_session_end.status, DreamStatus::Completed);
    assert!(completed_session_end.completed_at.is_some());
    let duplicate_after_complete = store
        .enqueue_dream(NewDreamQueueRecord::session_end(
            session.id,
            "brian",
            "global",
            Some(3),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate_after_complete.id, dream.id);
    assert_eq!(duplicate_after_complete.status, DreamStatus::Completed);

    let lifecycle = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:postgres-lifecycle:{}", Uuid::new_v4()),
            source_event_seq: None,
            available_at: Utc::now() - Duration::days(3650),
        })
        .await
        .unwrap();
    let claimed = store
        .claim_ready_dream(Utc::now(), Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres ready dream");
    assert_eq!(claimed.dream.id, lifecycle.id);
    assert_eq!(claimed.dream.status, DreamStatus::Running);
    assert_eq!(claimed.dream.attempts, 1);
    let heartbeat_at = Utc::now() + Duration::seconds(1);
    let heartbeated = store.heartbeat_dream(&claimed, heartbeat_at).await.unwrap();
    assert_postgres_timestamp_eq(heartbeated.dream.locked_at, Some(heartbeat_at));
    assert_postgres_timestamp_eq(heartbeated.dream.heartbeat_at, Some(heartbeat_at));
    let retry_at = Utc::now() + Duration::seconds(60);
    let retryable = store
        .fail_dream(
            &claimed,
            "postgres transient failure".to_string(),
            retry_at,
            2,
        )
        .await
        .unwrap();
    assert_eq!(retryable.status, DreamStatus::Queued);
    assert_eq!(
        retryable.last_error.as_deref(),
        Some("postgres transient failure")
    );
    assert!(retryable.error_at.is_some());
    let second = store
        .claim_ready_dream(retry_at, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres retry dream");
    assert_eq!(second.dream.id, lifecycle.id);
    assert_eq!(second.dream.attempts, 2);
    let completed = store.complete_dream(&second, Utc::now()).await.unwrap();
    assert_eq!(completed.status, DreamStatus::Completed);
    assert_eq!(completed.locked_at, None);
    assert!(completed.completed_at.is_some());
    assert_eq!(completed.error_at, None);

    let stale = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:postgres-stale:{}", Uuid::new_v4()),
            source_event_seq: None,
            available_at: Utc::now() - Duration::days(3650),
        })
        .await
        .unwrap();
    let stale_now = Utc::now();
    let first_stale_claim = store
        .claim_ready_dream(stale_now, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("postgres stale test dream");
    assert_eq!(first_stale_claim.dream.id, stale.id);
    assert_eq!(first_stale_claim.dream.attempts, 1);
    assert!(
        store
            .claim_ready_dream(
                stale_now + Duration::seconds(10),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none(),
        "fresh running lock must not be reclaimed"
    );
    let reclaimed = store
        .claim_ready_dream(
            stale_now + Duration::seconds(31),
            Duration::seconds(30),
            Uuid::new_v4(),
        )
        .await
        .unwrap()
        .expect("postgres stale dream reclaimed");
    assert_eq!(reclaimed.dream.id, stale.id);
    assert_eq!(reclaimed.dream.attempts, 2);
    assert!(
        store
            .complete_dream(&first_stale_claim, Utc::now())
            .await
            .is_err()
    );
    let terminal = store
        .fail_dream(
            &reclaimed,
            "postgres terminal failure".to_string(),
            Utc::now(),
            2,
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(
        terminal.last_error.as_deref(),
        Some("postgres terminal failure")
    );
    assert!(terminal.error_at.is_some());

    let cron_job_id = format!("postgres-lease-{}", Uuid::new_v4());
    let cron_now = Utc::now();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: cron_job_id.clone(),
            name: "Postgres lease test".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(cron_now),
        })
        .await
        .unwrap();
    let new_cron_run = NewCronRunRecord {
        job_id: cron_job_id.clone(),
        scheduled_for: cron_now,
        status: "queued".to_string(),
        session_id: None,
        result_json: json!({}),
    };
    let (first_cron, claimed) = store
        .claim_cron_run(
            new_cron_run.clone(),
            Uuid::new_v4(),
            cron_now,
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(claimed);
    let (reclaimed_cron, claimed) = store
        .claim_cron_run(
            new_cron_run.clone(),
            Uuid::new_v4(),
            cron_now + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(reclaimed_cron.run.id, first_cron.run.id);
    assert_eq!(reclaimed_cron.epoch, first_cron.epoch + 1);
    assert!(
        store
            .complete_cron_run(&first_cron, "completed", None, json!({}))
            .await
            .is_err()
    );
    let completed_cron = store
        .complete_cron_run(&reclaimed_cron, "completed", None, json!({}))
        .await
        .unwrap();
    assert_eq!(completed_cron.status, "completed");
    assert_eq!(
        store.record_cron_run(new_cron_run).await.unwrap().id,
        completed_cron.id
    );
    assert_eq!(store.cron_runs(&cron_job_id, 10).await.unwrap().len(), 1);

    let mut mode_state = session.mode_state.clone();
    mode_state.mode = ModeId::from("serious_engineer");
    mode_state.router_reason = Some("postgres replay test".to_string());
    mode_state.updated_at = Utc::now();
    let updated = store
        .set_mode_state(session.id, mode_state.clone())
        .await
        .unwrap();
    assert_eq!(updated.mode_state.mode, ModeId::from("serious_engineer"));

    let mode_event = store
        .append_event(
            session.id,
            "mode",
            serde_json::to_value(StoreEvent::ModeChanged(Box::new(ModeChangedStoreEvent {
                from: Some(ModeId::from("general")),
                mode: ModeId::from("serious_engineer"),
                label: "Serious Engineer".to_string(),
                capabilities: vec![
                    "fs.*".to_string(),
                    "code.*".to_string(),
                    "proc.*".to_string(),
                    "backend.coding".to_string(),
                ],
                active_skills: Vec::new(),
                router_reason: mode_state.router_reason,
                lock_source: None,
                override_source: None,
                updated_at: Utc::now(),
                persona_status: updated.persona_status,
            })))
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_event(
            session.id,
            "approval",
            json!({"approvalId": Uuid::new_v4(), "backend": "postgres-test"}),
        )
        .await
        .unwrap();
    store
        .append_event(
            session.id,
            "approval_resolved",
            json!({"backend": "postgres-test", "outcome": "selected"}),
        )
        .await
        .unwrap();
    let replay = store
        .events_after(session.id, Some(mode_event.seq))
        .await
        .unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["approval", "approval_resolved"]
    );

    let completed = store
        .append_event(
            session.id,
            "actor_completed",
            json!({
                "kind": "completed",
                "actor_id": "Worker",
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker",
            }),
        )
        .await
        .unwrap();
    assert_eq!(completed.actor_id.as_deref(), Some("Worker"));
    assert_eq!(completed.artifact_uri.as_deref(), Some("artifact://0"));
    assert_eq!(completed.history_uri.as_deref(), Some("history://Worker"));
    let replayed_completed = store
        .events_after(session.id, Some(completed.seq - 1))
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "actor_completed")
        .expect("actor_completed replay row");
    assert_eq!(replayed_completed.actor_id.as_deref(), Some("Worker"));
    assert_eq!(
        replayed_completed.artifact_uri.as_deref(),
        Some("artifact://0")
    );
    assert_eq!(
        replayed_completed.history_uri.as_deref(),
        Some("history://Worker")
    );

    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:postgres-test".to_string(),
            text: "memory row for P1 project view".to_string(),
            source: format!("session:{}", session.id),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let memory = store
        .recall_chunks("project:postgres-test", "P1 project", 5)
        .await
        .unwrap();
    assert_eq!(memory.len(), 1);

    let project_id = format!("postgres-test-{}", Uuid::new_v4());
    let item = store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: format!("project://{project_id}/artifacts/0"),
            source_session_id: session.id,
            source_event_seq: Some(2),
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "artifact://0".to_string(),
            provenance_json: json!({
                "sourceSession": session.id,
                "sourceEventId": 2,
                "sourceUri": "artifact://0",
            }),
        })
        .await
        .unwrap();
    let duplicate = store
        .upsert_project_item(NewProjectItem {
            project_id: project_id.clone(),
            kind: ProjectItemKind::Artifact,
            text: "artifact://0".to_string(),
            target_uri: format!("project://{project_id}/artifacts/0"),
            source_session_id: session.id,
            source_event_seq: Some(2),
            source_uri: Some("artifact://0".to_string()),
            dedupe_key: "artifact://0".to_string(),
            provenance_json: json!({"sourceSession": session.id}),
        })
        .await
        .unwrap();
    assert_eq!(item.id, duplicate.id);
    assert_eq!(
        store
            .project_items(&project_id, Some(ProjectItemKind::Artifact))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn gated_postgres_project_entity_lifecycle_archives_and_fails_closed() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p11_project_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let store = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();

    // §30: a project is a server-owned durable entity. ensure_project is idempotent on id.
    let created = store.ensure_project("atlas", "Atlas").await.unwrap();
    assert_eq!(created.id, "atlas");
    assert_eq!(created.title, "Atlas");
    assert_eq!(created.status, ProjectStatus::Active);
    assert!(created.archived_at.is_none());
    let again = store
        .ensure_project("atlas", "Atlas Renamed")
        .await
        .unwrap();
    assert_eq!(again.id, created.id);
    assert_eq!(again.status, ProjectStatus::Active);
    assert_eq!(store.project("atlas").await.unwrap().unwrap().id, "atlas");
    assert_eq!(
        store
            .projects(false)
            .await
            .unwrap()
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>(),
        vec!["atlas".to_string()]
    );

    // Project-scoped memory persists while the entity is active.
    let scope = "project:atlas";
    let record = durable_episodic_record(
        Uuid::new_v4(),
        "brian",
        scope,
        "Atlas retains this until archive.",
        MemoryRecordStatus::Active,
        MemoryRecordLinks::default(),
    );
    let record = store.upsert_memory_record(record).await.unwrap();
    store
        .enqueue_memory_embedding_job(pending_embedding_job(&record))
        .await
        .unwrap();
    assert_eq!(
        store
            .active_memory_records("brian", scope, 5)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .memory_scope_tombstone("brian", scope)
            .await
            .unwrap()
            .is_none()
    );

    // §30.4: archive flips status and commits the memory-scope tombstone in one transaction so
    // exact recall/replay fails closed immediately after archive.
    let archived = store
        .archive_project("brian", "atlas", "wrapped up")
        .await
        .unwrap();
    assert_eq!(archived.status, ProjectStatus::Archived);
    assert!(archived.archived_at.is_some());
    assert!(
        store
            .memory_scope_tombstone("brian", scope)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        store.active_memory_records("brian", scope, 5).await,
        Err(ServerError::NotFound(_))
    ));
    assert!(matches!(
        store.memory_embedding_jobs("brian", scope).await,
        Err(ServerError::NotFound(_))
    ));

    // Archived entities drop out of the default listing but remain visible with include_archived.
    assert!(store.projects(false).await.unwrap().is_empty());
    assert_eq!(
        store
            .projects(true)
            .await
            .unwrap()
            .iter()
            .map(|project| (project.id.clone(), project.status))
            .collect::<Vec<_>>(),
        vec![("atlas".to_string(), ProjectStatus::Archived)]
    );
    // ensure_project never resurrects an archived entity (§30 lifecycle).
    let after = store.ensure_project("atlas", "Atlas").await.unwrap();
    assert_eq!(after.status, ProjectStatus::Archived);

    drop(store);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
