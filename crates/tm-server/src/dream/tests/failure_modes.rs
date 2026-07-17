use super::support::*;
use super::*;

#[tokio::test]
async fn dream_worker_timeout_failure_is_replayable_and_bounded() {
    let store = Arc::new(InMemoryStore::default());
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
        .append_message(session.id, "user", "Summarize this before the timeout.")
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:timeout:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            per_dream_timeout: StdDuration::ZERO,
            retry_backoff: Duration::zero(),
            max_attempts: 2,
            ..DreamWorkerConfig::default()
        },
    );

    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.attempted, 1);
    assert_eq!(first.completed, 0);
    let retryable = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(retryable[0].status, DreamStatus::Queued);
    assert_eq!(retryable[0].attempts, 1);
    assert_eq!(
        retryable[0].last_error.as_deref(),
        Some("store error: dream timed out")
    );

    let second = worker.run_once_result().await.unwrap();
    assert_eq!(second.attempted, 1);
    assert_eq!(second.completed, 0);
    let terminal = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(terminal[0].status, DreamStatus::Failed);
    assert_eq!(terminal[0].attempts, 2);
    assert_eq!(
        terminal[0].last_error.as_deref(),
        Some("store error: dream timed out")
    );

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_started")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_failed")
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "dream_completed")
    );
}

#[tokio::test]
async fn dream_worker_redaction_disabled_fails_visibly_without_writes() {
    let store = Arc::new(InMemoryStore::default());
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
        .append_message(
            session.id,
            "user",
            "Remember that I prefer this should not be written when redaction is disabled.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:redaction-disabled:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            redaction: DreamRedactionConfig { enabled: false },
            retry_backoff: Duration::zero(),
            max_attempts: 1,
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 0);
    assert_eq!(report.proposals, 0);
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Failed);
    assert_eq!(
        dreams[0].last_error.as_deref(),
        Some("policy error: dream redaction is disabled by config")
    );
    assert!(
        store
            .memory_summaries("global", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_failed")
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
}

#[tokio::test]
async fn dream_worker_missing_model_role_fails_visibly_without_writes() {
    let store = Arc::new(InMemoryStore::default());
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
        .append_message(session.id, "user", "Decision: keep model config explicit.")
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:missing-model-role:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let mut model_roles = DreamModelRoles::default();
    model_roles.extraction.clear();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            model_roles,
            retry_backoff: Duration::zero(),
            max_attempts: 1,
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 0);
    assert_eq!(report.proposals, 0);
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Failed);
    assert_eq!(
        dreams[0].last_error.as_deref(),
        Some("policy error: dream model role extraction is not configured")
    );
    assert!(
        store
            .memory_summaries("global", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .recall_chunks("global", "model config", 10)
            .await
            .unwrap()
            .is_empty()
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_failed")
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
}
