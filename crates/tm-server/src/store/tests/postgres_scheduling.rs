use super::*;

#[tokio::test]
async fn gated_postgres_cron_materialization_and_leasing_are_cross_instance_safe() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let first = PostgresStore::connect(&dsn).await.unwrap();
    let second = PostgresStore::connect(&dsn).await.unwrap();
    let job_id = format!("postgres-cron-cas-{}", Uuid::new_v4());
    let due = Utc::now() - Duration::minutes(1);
    let next = due + Duration::minutes(1);
    first
        .upsert_cron_job(NewCronJobRecord {
            id: job_id.clone(),
            name: "cross instance cron CAS".to_string(),
            schedule: "* * * * *".to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 1,
            script_timeout_seconds: 30,
            next_run_at: Some(due),
        })
        .await
        .unwrap();
    let new_run = NewCronRunRecord {
        job_id: job_id.clone(),
        scheduled_for: due,
        status: "queued".to_string(),
        session_id: None,
        result_json: json!({"source": "cross-instance-test"}),
    };
    let (left, right) = tokio::join!(
        first.materialize_cron_run(new_run.clone(), due, next),
        second.materialize_cron_run(new_run, due, next),
    );
    assert_ne!(left.unwrap().is_some(), right.unwrap().is_some());
    assert_eq!(first.cron_runs(&job_id, 10).await.unwrap().len(), 1);
    assert_postgres_timestamp_eq(
        first.cron_job(&job_id).await.unwrap().next_run_at,
        Some(next),
    );

    let claimed_at = Utc::now();
    let first_owner = Uuid::new_v4();
    let second_owner = Uuid::new_v4();
    let (left, right) = tokio::join!(
        first.claim_ready_cron_run(first_owner, claimed_at, Duration::seconds(60), 3),
        second.claim_ready_cron_run(second_owner, claimed_at, Duration::seconds(60), 3),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.is_some(), right.is_some());
    let original = left.or(right).unwrap();
    let reclaiming_store = if original.owner_id == first_owner {
        &second
    } else {
        &first
    };
    let reclaimed = reclaiming_store
        .claim_ready_cron_run(
            Uuid::new_v4(),
            claimed_at + Duration::seconds(61),
            Duration::seconds(60),
            3,
        )
        .await
        .unwrap()
        .expect("stale cross-instance lease is reclaimed");
    assert_eq!(reclaimed.run.id, original.run.id);
    assert_eq!(reclaimed.epoch, original.epoch + 1);
    assert!(
        first
            .complete_cron_run(&original, "completed", None, json!({}))
            .await
            .is_err(),
        "the stale owner must be fenced"
    );
    let third = first
        .claim_ready_cron_run(
            Uuid::new_v4(),
            claimed_at + Duration::seconds(122),
            Duration::seconds(60),
            3,
        )
        .await
        .unwrap()
        .expect("third and final attempt is leased");
    assert_eq!(third.run.attempts, 3);
    assert!(
        second
            .claim_ready_cron_run(
                Uuid::new_v4(),
                claimed_at + Duration::seconds(183),
                Duration::seconds(60),
                3,
            )
            .await
            .unwrap()
            .is_none()
    );
    let terminal = first
        .cron_runs(&job_id, 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(terminal.status, "failed");
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.completed_at.is_some());
}

#[tokio::test]
async fn gated_postgres_dream_crash_on_final_attempt_is_fenced_and_terminal() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let first = PostgresStore::connect(&dsn).await.unwrap();
    let second = PostgresStore::connect(&dsn).await.unwrap();
    let session = first
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let now = Utc::now();
    let dream = first
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:postgres-final-crash:{}", Uuid::new_v4()),
            source_event_seq: None,
            available_at: now,
        })
        .await
        .unwrap();

    let first_lease = first
        .claim_ready_dream_bounded(now, Duration::seconds(30), Uuid::new_v4(), 3)
        .await
        .unwrap()
        .unwrap();
    let second_lease = second
        .claim_ready_dream_bounded(
            now + Duration::seconds(31),
            Duration::seconds(30),
            Uuid::new_v4(),
            3,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_lease.epoch, first_lease.epoch + 1);
    let third_lease = first
        .claim_ready_dream_bounded(
            now + Duration::seconds(62),
            Duration::seconds(30),
            Uuid::new_v4(),
            3,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third_lease.dream.attempts, 3);
    if let Some(unrelated) = second
        .claim_ready_dream_bounded(
            now + Duration::seconds(93),
            Duration::seconds(30),
            Uuid::new_v4(),
            3,
        )
        .await
        .unwrap()
    {
        assert_ne!(unrelated.dream.id, dream.id);
        second
            .complete_dream(&unrelated, now + Duration::seconds(93))
            .await
            .unwrap();
    }
    let terminal = first.dream(dream.id).await.unwrap();
    assert_eq!(terminal.status, DreamStatus::Failed);
    assert_eq!(terminal.attempts, 3);
    assert_eq!(terminal.lease_owner, None);
    assert!(terminal.error_at.is_some());
}
