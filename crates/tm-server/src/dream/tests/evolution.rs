use super::support::test_sender_factory;
use super::*;
use crate::dream::evolution::{capture_episodes, value_episodes};
use tm_memory::{EpisodeStatus, RewardSource, TraceKind};

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
    assert_eq!(episode.status, EpisodeStatus::Valued);
    assert_eq!(episode.terminal_reward, Some(0.5));
    assert_eq!(episode.reward_source, Some(RewardSource::Runtime));
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
    assert_eq!(traces[0].value, Some(0.46375));
    assert_eq!(traces[1].value, Some(0.475));
    assert_eq!(traces[2].value, Some(0.5));

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
    let valuation_progress = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("evolution_valued")
        })
        .expect("valuation progress");
    assert_eq!(valuation_progress.payload_json["episodes"], json!(1));

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
async fn dream_valuation_failure_retries_without_stranding_captured_episode() {
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
        .enqueue_turn(session.id, "valuation-failure", "inspect")
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_start",
            json!({"nodeId": "node-1", "capability": "fs.read", "argsPreview": "config"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_result",
            json!({"nodeId": "node-1", "status": "completed", "resultPreview": "done"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(session.id, "final", json!({"text": "done"}), Some(turn.id))
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
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:valuation-failure:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    store.fail_next_set_episode_valuation();
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            retry_backoff: Duration::zero(),
            proposal_timeout: StdDuration::from_millis(20),
            ..DreamWorkerConfig::default()
        },
    );

    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.completed, 1);
    assert_eq!(
        store
            .evolution_episode_for_turn(turn.id)
            .await
            .unwrap()
            .expect("captured episode")
            .status,
        EpisodeStatus::Captured
    );
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].status, DreamStatus::Completed);
    assert_eq!(dreams[1].status, DreamStatus::Queued);
    assert_eq!(
        dreams[1].dedupe_key,
        format!("dream:evolution-valuation:{}", session.id)
    );
    let skipped = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("evolution_skipped")
        })
        .expect("valuation skip progress");
    assert!(
        skipped.payload_json["reason"]
            .as_str()
            .unwrap()
            .contains("simulated episode valuation failure")
    );

    let second = worker.run_once_result().await.unwrap();
    assert_eq!(second.completed, 1);
    let episode = store
        .evolution_episode_for_turn(turn.id)
        .await
        .unwrap()
        .expect("valued episode");
    assert_eq!(episode.status, EpisodeStatus::Valued);
    assert_eq!(episode.terminal_reward, Some(0.5));
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].id, dream.id);
    assert_eq!(dreams[1].status, DreamStatus::Completed);
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

async fn captured_episode_fixture(
    store: &Arc<InMemoryStore>,
    client_message_id: &str,
    terminal_type: &str,
    failed_cell: bool,
) -> (
    SessionRecord,
    SessionTurnRecord,
    tm_memory::EvolutionEpisodeRecord,
    Vec<SessionEvent>,
) {
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, client_message_id, "inspect")
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_start",
            json!({"nodeId": "node-1", "capability": "fs.read", "argsPreview": "config"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "effect_result",
            json!({"nodeId": "node-1", "status": "completed", "resultPreview": "done"}),
            Some(turn.id),
        )
        .await
        .unwrap();
    if failed_cell {
        store
            .append_event_for_turn(
                session.id,
                "cell_result",
                json!({"cellId": "cell-1", "status": "failed", "error": "[redacted]"}),
                Some(turn.id),
            )
            .await
            .unwrap();
    }
    store
        .append_event_for_turn(
            session.id,
            terminal_type,
            if terminal_type == "final" {
                json!({"text": "done"})
            } else {
                json!({"message": "failed"})
            },
            Some(turn.id),
        )
        .await
        .unwrap();
    let dream = DreamQueueRecord {
        id: Uuid::new_v4(),
        session_id: session.id,
        subject: "brian".to_string(),
        scope: "global".to_string(),
        reason: DreamReason::ManualReflect,
        status: DreamStatus::Queued,
        dedupe_key: format!("dream:valuation:{client_message_id}"),
        source_event_seq: None,
        attempts: 0,
        enqueued_at: Utc::now(),
        available_at: Utc::now(),
        locked_at: None,
        lease_owner: None,
        lease_epoch: 0,
        heartbeat_at: None,
        completed_at: None,
        error_at: None,
        last_error: None,
    };
    let events = store.events_after(session.id, None).await.unwrap();
    let sink = StoreCodingEventSink::new(
        session.id,
        Arc::clone(store),
        test_sender_factory()(session.id),
    );
    let episodes = capture_episodes(
        store,
        &EvolutionDreamConfig::default(),
        &dream,
        &events,
        &sink,
    )
    .await
    .unwrap();
    (session, turn, episodes[0].clone(), events)
}

#[tokio::test]
async fn dream_valuation_prefers_explicit_feedback_and_skips_valued_episodes() {
    let store = Arc::new(InMemoryStore::default());
    let (session, turn, episode, events) =
        captured_episode_fixture(&store, "explicit-feedback", "final", false).await;
    store
        .record_turn_feedback(
            session.id,
            turn.id,
            tm_memory::FeedbackOutcome::Corrected,
            None,
        )
        .await
        .unwrap();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: "dream:valuation:explicit".to_string(),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let sink = StoreCodingEventSink::new(
        session.id,
        Arc::clone(&store),
        test_sender_factory()(session.id),
    );
    let valued = value_episodes(
        &store,
        &EvolutionDreamConfig::default(),
        &dream,
        std::slice::from_ref(&episode),
        &events,
        &sink,
    )
    .await
    .unwrap();
    assert_eq!(valued.len(), 1);
    assert_eq!(valued[0].terminal_reward, Some(-0.4));
    assert_eq!(valued[0].reward_source, Some(RewardSource::Explicit));
    assert_eq!(
        valued[0].feedback_outcome,
        Some(tm_memory::FeedbackOutcome::Corrected)
    );
    assert!(
        store
            .experience_traces(episode.id)
            .await
            .unwrap()
            .iter()
            .all(|trace| trace.value.is_some())
    );
    assert!(
        value_episodes(
            &store,
            &EvolutionDreamConfig::default(),
            &dream,
            &valued,
            &events,
            &sink,
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn dream_valuation_maps_runtime_terminal_outcomes() {
    for (name, terminal_type, failed_cell, expected) in [
        ("clean-final", "final", false, 0.5),
        ("failed-cell", "final", true, -0.2),
        ("terminal-error", "error", false, -0.6),
    ] {
        let store = Arc::new(InMemoryStore::default());
        let (session, _, episode, events) =
            captured_episode_fixture(&store, name, terminal_type, failed_cell).await;
        let dream = store
            .enqueue_dream(NewDreamQueueRecord {
                session_id: session.id,
                subject: "brian".to_string(),
                scope: "global".to_string(),
                reason: DreamReason::ManualReflect,
                dedupe_key: format!("dream:valuation:runtime:{name}"),
                source_event_seq: None,
                available_at: Utc::now(),
            })
            .await
            .unwrap();
        let sink = StoreCodingEventSink::new(
            session.id,
            Arc::clone(&store),
            test_sender_factory()(session.id),
        );
        let valued = value_episodes(
            &store,
            &EvolutionDreamConfig::default(),
            &dream,
            &[episode],
            &events,
            &sink,
        )
        .await
        .unwrap();
        assert_eq!(valued[0].terminal_reward, Some(expected), "{name}");
        assert_eq!(valued[0].reward_source, Some(RewardSource::Runtime));
        assert_eq!(valued[0].feedback_outcome, None);
    }
}
