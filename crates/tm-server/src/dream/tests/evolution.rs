use super::support::test_sender_factory;
use super::*;
use tm_memory::{EpisodeStatus, TraceKind};

#[tokio::test]
async fn dream_capture_pairs_bounded_traces_and_is_idempotent() {
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
    let worker_id = Uuid::new_v4();
    let turn = store
        .enqueue_turn(session.id, "evolution-capture", "inspect")
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "cell_start",
            json!({"cellId": "cell-1", "sourcePreview": "read config"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_start",
            json!({"cellId": "cell-1", "nodeId": "node-1", "capability": "fs.read", "argsPreview": "x".repeat(40)}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_result",
            json!({"cellId": "cell-1", "nodeId": "node-1", "status": "failed", "error": "ENOENT /tmp/private/123"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_start",
            json!({"cellId": "cell-1", "nodeId": "node-2", "capability": "code.symbols", "argsPreview": "unpaired"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "cell_result",
            json!({"cellId": "cell-1", "status": "completed", "resultPreview": "done"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "final",
            json!({"text": "finished successfully"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .expect("queued turn");
    store
        .complete_turn(turn.id, worker_id, "finished successfully", Utc::now())
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(session.id, "session_end", json!({"status": "ended"}))
        .await
        .unwrap();
    let new_dream = NewDreamQueueRecord {
        session_id: session.id,
        subject: "brian".to_string(),
        scope: "global".to_string(),
        reason: DreamReason::SessionEnded,
        dedupe_key: format!("dream:evolution:{}", session.id),
        source_event_seq: Some(ended.seq),
        available_at: Utc::now(),
    };
    let dream = store.enqueue_dream(new_dream.clone()).await.unwrap();
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            evolution: EvolutionDreamConfig {
                max_traces_per_episode: 3,
                max_trace_field_chars: 16,
                ..EvolutionDreamConfig::default()
            },
            proposal_timeout: StdDuration::from_millis(20),
            ..DreamWorkerConfig::default()
        },
    );

    let report = worker.run_once_result().await.unwrap();
    assert_eq!(report.completed, 1);
    let episode = store
        .evolution_episode_for_turn(turn.id)
        .await
        .unwrap()
        .expect("captured episode");
    assert_eq!(episode.status, EpisodeStatus::Captured);
    assert_eq!(episode.trace_count, 3);
    let traces = store.experience_traces(episode.id).await.unwrap();
    assert_eq!(
        traces.iter().map(|trace| trace.kind).collect::<Vec<_>>(),
        vec![TraceKind::Effect, TraceKind::Effect, TraceKind::Terminal]
    );
    assert_eq!(traces[0].capability.as_deref(), Some("fs.read"));
    assert_eq!(traces[0].action_summary.chars().count(), 16);
    assert_eq!(traces[0].error_signature.as_deref(), Some("enoent"));
    assert_eq!(traces[1].observation_summary, "unresolved");
    assert_eq!(traces[2].observation_summary, "finished success");
    assert!(
        traces
            .iter()
            .all(|trace| trace.action_summary.chars().count() <= 16)
    );
    assert_eq!(traces[2].event_seq, ended.seq - 1);

    let progress = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("evolution_episodes_captured")
        })
        .expect("evolution progress");
    assert_eq!(progress.payload_json["episodes"], json!(1));
    assert_eq!(progress.payload_json["traces"], json!(3));

    let before = traces;
    store
        .enqueue_dream(NewDreamQueueRecord {
            dedupe_key: format!("dream:evolution:rerun:{}", session.id),
            ..new_dream
        })
        .await
        .unwrap();
    worker.run_once_result().await.unwrap();
    assert_eq!(store.experience_traces(episode.id).await.unwrap(), before);
    assert_eq!(
        store
            .evolution_episodes("brian", "global", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store.dream_queue_for_session(session.id).await.unwrap()[0].id,
        dream.id
    );
}

#[tokio::test]
async fn dream_capture_emits_zero_counts_for_no_terminal_turns() {
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
    let worker_id = Uuid::new_v4();
    let turn = store
        .enqueue_turn(session.id, "evolution-no-terminal", "inspect")
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "cell_start",
            json!({"cellId": "cell-1", "sourcePreview": "[redacted]"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .claim_next_turn(worker_id, Utc::now())
        .await
        .unwrap()
        .expect("queued turn");
    store
        .complete_turn(turn.id, worker_id, "done", Utc::now())
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
            dedupe_key: format!("dream:evolution:none:{}", session.id),
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
            ..DreamWorkerConfig::default()
        },
    );

    worker.run_once_result().await.unwrap();
    assert!(
        store
            .evolution_episode_for_turn(turn.id)
            .await
            .unwrap()
            .is_none()
    );
    let progress = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("evolution_episodes_captured")
        })
        .expect("zero-count progress");
    assert_eq!(progress.payload_json["episodes"], json!(0));
    assert_eq!(progress.payload_json["traces"], json!(0));
}
