use super::support::*;
use super::*;

#[tokio::test]
async fn dream_worker_daemon_processes_queue_and_stops_cleanly() {
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
            "Close this session with a compact summary.",
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
            dedupe_key: format!("dream:daemon:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let daemon = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            poll_interval: Duration::milliseconds(5),
            ..DreamWorkerConfig::default()
        },
    )
    .into_daemon();
    let handle = daemon.spawn();
    wait_for_event_count(&store, session.id, "dream_completed", 1).await;
    handle.request_shutdown();
    handle.shutdown().await.unwrap();

    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Completed);
    assert_eq!(dreams[0].locked_at, None);
}

#[tokio::test]
async fn dream_worker_budgets_redacted_input_before_summary_and_proposals() {
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
            "Remember that I prefer compact dream summaries.",
        )
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "user",
            "token=sk-testsecret123456 and this long note should be trimmed before it can sprawl through the dream input collector.",
        )
        .await
        .unwrap();
    for index in 0..12 {
        store
            .append_message(
                session.id,
                "user",
                &format!(
                    "Remember that I prefer omitted preference {index}. {}",
                    "filler ".repeat(20)
                ),
            )
            .await
            .unwrap();
    }
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:budget:{}", session.id),
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
            proposal_timeout: StdDuration::from_millis(20),
            input_budget: DreamInputBudget {
                max_chunks: 1,
                max_chunk_chars: 220,
                max_message_chars: 80,
            },
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 1);
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let session_summary = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Session)
        .expect("session summary");
    assert_eq!(session_summary.source_dream_id, dream.id);
    assert!(
        session_summary
            .evidence
            .iter()
            .any(|evidence| evidence.event_seq == Some(ended.seq))
    );
    assert!(session_summary.body.contains("Input budget: included"));
    assert!(!session_summary.body.contains("sk-testsecret123456"));

    let events = wait_for_event_count(&store, session.id, "approval_resolved", 1).await;
    let input = events
        .iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("input_collected")
        })
        .unwrap();
    assert_eq!(input.payload_json["totalMessages"], json!(14));
    assert!(input.payload_json["omittedMessages"].as_u64().unwrap() > 0);
    assert!(input.payload_json["truncatedMessages"].as_u64().unwrap() > 0);
    assert_eq!(input.payload_json["inputChunks"], json!(1));
    assert_eq!(input.payload_json["inputTruncated"], json!(true));
    assert_eq!(input.payload_json["redactedMessages"], json!(1));
    assert_eq!(
        store.profile_facts("brian").await.unwrap(),
        Vec::new(),
        "timed-out budgeted proposal must not write profile facts"
    );
    let proposal_events = events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .collect::<Vec<_>>();
    assert_eq!(proposal_events.len(), 2);
    assert!(proposal_events.iter().all(|event| {
        !event
            .payload_json
            .to_string()
            .contains("omitted preference")
    }));
}
