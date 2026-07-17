use super::support::*;
use super::*;

#[tokio::test]
async fn dream_worker_writes_reflection_when_importance_crosses_threshold() {
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
            "Deadline: send the P4 notes by Friday.\nDecision: keep reflection summaries evidence-cited.",
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
            dedupe_key: format!("dream:reflection:{}", session.id),
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
            reflect_importance_threshold: 1.5,
            ..DreamWorkerConfig::default()
        },
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 2);
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let reflection = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Reflection)
        .expect("reflection summary");
    assert!(reflection.body.contains("Cumulative importance"));
    assert!(reflection.body.contains("Deadline: send the P4 notes"));
    assert!(
        reflection
            .body
            .contains("Decision: keep reflection summaries evidence-cited")
    );
    assert!(reflection.body.contains("Evidence citations"));
    assert_eq!(reflection.source_session_id, Some(session.id));
    assert!(
        reflection
            .evidence
            .iter()
            .any(|evidence| evidence.event_seq == Some(ended.seq))
    );

    let events = wait_for_event_count(&store, session.id, "approval_resolved", 2).await;
    assert!(events.iter().any(|event| {
        event.event_type == "dream_progress"
            && event.payload_json["phase"] == json!("reflection_written")
    }));
}

#[tokio::test]
async fn dream_worker_updates_recursive_rollup_from_recent_summaries() {
    let store = Arc::new(InMemoryStore::default());
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            reflect_importance_threshold: 100.0,
            ..DreamWorkerConfig::default()
        },
    );

    for (index, content) in [
        "Please summarize release checkpoint one: SSE replay remained stable.",
        "Please summarize release checkpoint two: mobile smoke is next.",
    ]
    .into_iter()
    .enumerate()
    {
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
            .append_message(session.id, "user", content)
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
                dedupe_key: format!("dream:rollup:{index}:{}", session.id),
                source_event_seq: Some(ended.seq),
                available_at: Utc::now(),
            })
            .await
            .unwrap();

        let report = worker.run_once_result().await.unwrap();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.proposals, 0);
    }

    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let rollups = summaries
        .iter()
        .filter(|summary| summary.kind == MemorySummaryKind::TopicProject)
        .collect::<Vec<_>>();
    assert_eq!(rollups.len(), 1);
    let rollup = rollups[0];
    assert_eq!(rollup.title, "Rollup: global");
    assert!(rollup.body.contains("Folded summaries: 2"));
    assert!(rollup.body.contains("release checkpoint one"));
    assert!(rollup.body.contains("release checkpoint two"));
    assert!(rollup.evidence.iter().all(|evidence| {
        evidence
            .uri
            .as_deref()
            .is_some_and(|uri| uri.starts_with("memory://summaries/"))
    }));
}

#[tokio::test]
async fn dream_worker_distress_only_session_writes_summary_without_durable_proposals() {
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
            "I'm overwhelmed and sad tonight. Please just sit with me for a minute.",
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
            dedupe_key: format!("dream:distress:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 0);
    assert_eq!(store.profile_facts("brian").await.unwrap(), Vec::new());
    assert_eq!(
        store
            .recall_chunks("global", "overwhelmed", 10)
            .await
            .unwrap(),
        Vec::new()
    );
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let session_summary = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Session)
        .expect("session summary");
    assert!(session_summary.body.contains("I'm overwhelmed and sad"));
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_completed")
    );
}
