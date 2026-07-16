use super::*;

#[tokio::test]
async fn in_memory_dream_claim_heartbeat_complete_and_stale_reclaim() {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let now = Utc::now();
    let queued = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:lifecycle:{}", session.id),
            source_event_seq: None,
            available_at: now - Duration::seconds(1),
        })
        .await
        .unwrap();

    let first_owner = Uuid::new_v4();
    let claimed = store
        .claim_ready_dream(now, Duration::seconds(30), first_owner)
        .await
        .unwrap()
        .expect("ready dream");
    assert_eq!(claimed.dream.id, queued.id);
    assert_eq!(claimed.dream.status, DreamStatus::Running);
    assert_eq!(claimed.dream.attempts, 1);
    assert_eq!(claimed.dream.locked_at, Some(now));
    assert_eq!(claimed.dream.heartbeat_at, Some(now));
    assert!(
        store
            .claim_ready_dream(
                now + Duration::seconds(10),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );

    let reclaimed = store
        .claim_ready_dream(
            now + Duration::seconds(31),
            Duration::seconds(30),
            Uuid::new_v4(),
        )
        .await
        .unwrap()
        .expect("stale running dream");
    assert_eq!(reclaimed.dream.id, queued.id);
    assert_eq!(reclaimed.dream.status, DreamStatus::Running);
    assert_eq!(reclaimed.dream.attempts, 2);
    assert!(store.complete_dream(&claimed, Utc::now()).await.is_err());

    let heartbeat_at = now + Duration::seconds(35);
    let heartbeated = store
        .heartbeat_dream(&reclaimed, heartbeat_at)
        .await
        .unwrap();
    assert_eq!(heartbeated.dream.locked_at, Some(heartbeat_at));
    assert_eq!(heartbeated.dream.heartbeat_at, Some(heartbeat_at));

    let completed_at = now + Duration::seconds(40);
    let completed = store
        .complete_dream(&reclaimed, completed_at)
        .await
        .unwrap();
    assert_eq!(completed.status, DreamStatus::Completed);
    assert_eq!(completed.locked_at, None);
    assert_eq!(completed.available_at, completed_at);
    assert_eq!(completed.completed_at, Some(completed_at));
    assert_eq!(completed.error_at, None);
    assert!(
        store
            .claim_ready_dream(
                now + Duration::seconds(90),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_dream_crash_on_final_attempt_becomes_terminal() {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let now = Utc::now();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:final-crash:{}", session.id),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();

    for attempt in 1..=3 {
        let lease = store
            .claim_ready_dream_bounded(
                now + Duration::seconds(i64::from((attempt - 1) * 31)),
                Duration::seconds(30),
                Uuid::new_v4(),
                3,
            )
            .await
            .unwrap()
            .expect("attempt should be leased");
        assert_eq!(lease.dream.attempts, attempt);
    }
    assert!(
        store
            .claim_ready_dream_bounded(
                now + Duration::seconds(93),
                Duration::seconds(30),
                Uuid::new_v4(),
                3,
            )
            .await
            .unwrap()
            .is_none()
    );
    let terminal = store.dream(dream.id).await.unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.error_at.is_some());
    assert!(
        terminal
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("final permitted attempt"))
    );
}

#[tokio::test]
async fn in_memory_dream_failure_records_error_and_bounded_retry() {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let now = Utc::now();
    let _queued = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:retry:{}", session.id),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();

    let first = store
        .claim_ready_dream(now, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("first attempt");
    assert_eq!(first.dream.attempts, 1);

    let retry_at = now + Duration::seconds(60);
    let retryable = store
        .fail_dream(&first, "model timeout".to_string(), retry_at, 2)
        .await
        .unwrap();
    assert_eq!(retryable.status, DreamStatus::Queued);
    assert_eq!(retryable.attempts, 1);
    assert_eq!(retryable.last_error.as_deref(), Some("model timeout"));
    assert_eq!(retryable.available_at, retry_at);
    assert!(retryable.error_at.is_some());
    assert_eq!(retryable.completed_at, None);
    assert!(
        store
            .claim_ready_dream(
                now + Duration::seconds(30),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );

    let second = store
        .claim_ready_dream(retry_at, Duration::seconds(30), Uuid::new_v4())
        .await
        .unwrap()
        .expect("second attempt");
    assert_eq!(second.dream.attempts, 2);

    let terminal = store
        .fail_dream(
            &second,
            "redaction failed".to_string(),
            retry_at + Duration::seconds(60),
            2,
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.locked_at, None);
    assert_eq!(terminal.last_error.as_deref(), Some("redaction failed"));
    assert!(terminal.error_at.is_some());
    assert!(
        store
            .claim_ready_dream(
                retry_at + Duration::seconds(120),
                Duration::seconds(30),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_cron_claim_is_unique_heartbeat_aware_and_fenced() {
    let store = InMemoryStore::default();
    let now = Utc::now();
    let job = store
        .upsert_cron_job(NewCronJobRecord {
            id: "lease-test".to_string(),
            name: "Lease test sk-testsecret123456".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(now),
        })
        .await
        .unwrap();
    assert!(!job.name.contains("sk-testsecret123456"));
    let new_run = NewCronRunRecord {
        job_id: "lease-test".to_string(),
        scheduled_for: now,
        status: "queued".to_string(),
        session_id: None,
        result_json: json!({"accessToken": "short-secret"}),
    };
    let (first, claimed) = store
        .claim_cron_run(new_run.clone(), Uuid::new_v4(), now, Duration::seconds(30))
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(first.epoch, 1);
    assert_eq!(first.run.attempts, 1);
    assert_eq!(first.run.result_json["accessToken"], "[REDACTED_SECRET]");

    let heartbeat = store
        .heartbeat_cron_run(&first, now + Duration::seconds(20))
        .await
        .unwrap();
    let (still_owned, claimed) = store
        .claim_cron_run(
            new_run.clone(),
            Uuid::new_v4(),
            now + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(!claimed);
    assert_eq!(still_owned.owner_id, heartbeat.owner_id);

    let (reclaimed, claimed) = store
        .claim_cron_run(
            new_run.clone(),
            Uuid::new_v4(),
            now + Duration::seconds(51),
            Duration::seconds(30),
        )
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(reclaimed.run.id, first.run.id);
    assert_eq!(reclaimed.epoch, 2);
    assert_eq!(reclaimed.run.attempts, 2);
    assert!(
        store
            .complete_cron_run(&first, "completed", None, json!({}))
            .await
            .is_err()
    );
    let completed = store
        .complete_cron_run(
            &reclaimed,
            "completed",
            None,
            json!({"message": "Authorization: Bearer opaque-token-123456"}),
        )
        .await
        .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.lease_owner, None);
    assert!(
        !completed
            .result_json
            .to_string()
            .contains("opaque-token-123456")
    );

    let recorded = store.record_cron_run(new_run).await.unwrap();
    assert_eq!(recorded.id, completed.id);
    assert_eq!(store.cron_runs("lease-test", 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn in_memory_cron_crash_on_final_attempt_becomes_terminal() {
    let store = InMemoryStore::default();
    let now = Utc::now();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: "final-crash".to_string(),
            name: "Final crash".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(now),
        })
        .await
        .unwrap();
    let materialized = store
        .materialize_cron_run(
            NewCronRunRecord {
                job_id: "final-crash".to_string(),
                scheduled_for: now,
                status: "queued".to_string(),
                session_id: None,
                result_json: json!({}),
            },
            now,
            now + Duration::minutes(1),
        )
        .await
        .unwrap()
        .unwrap();
    let claim_base = materialized.available_at;

    let mut run_id = None;
    for attempt in 1..=3 {
        let lease = store
            .claim_ready_cron_run(
                Uuid::new_v4(),
                claim_base + Duration::seconds(i64::from((attempt - 1) * 31)),
                Duration::seconds(30),
                3,
            )
            .await
            .unwrap()
            .expect("attempt should be leased");
        run_id = Some(lease.run.id);
        assert_eq!(lease.run.attempts, attempt);
    }
    assert!(
        store
            .claim_ready_cron_run(
                Uuid::new_v4(),
                claim_base + Duration::seconds(93),
                Duration::seconds(30),
                3,
            )
            .await
            .unwrap()
            .is_none()
    );
    let terminal = store
        .cron_runs("final-crash", 10)
        .await
        .unwrap()
        .into_iter()
        .find(|run| Some(run.id) == run_id)
        .unwrap();
    assert_eq!(terminal.status, "failed");
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.completed_at.is_some());
    assert!(
        terminal
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("final permitted attempt"))
    );
}

#[tokio::test]
async fn in_memory_runtime_metrics_report_queue_age_and_scheduler_lag() {
    let store = InMemoryStore::default();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .enqueue_turn(session.id, "metrics-turn", "queued")
        .await
        .unwrap();
    let now = Utc::now();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:metrics:{}", session.id),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: "metrics".to_string(),
            name: "Metrics".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(now - Duration::seconds(45)),
        })
        .await
        .unwrap();

    let metrics = store
        .runtime_metrics(now + Duration::seconds(5))
        .await
        .unwrap();
    assert_eq!(metrics.turn_queue_depth, 1);
    assert_eq!(metrics.dream_queue_depth, 1);
    assert_eq!(metrics.cron_queue_depth, 0);
    assert!(metrics.scheduler_lag_seconds >= 50);
    assert_eq!(metrics.pending_approvals, 0);
    assert_eq!(metrics.lease_reclaims, 0);
}
