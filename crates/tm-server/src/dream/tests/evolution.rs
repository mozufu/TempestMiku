use super::support::test_sender_factory;
use super::*;
use crate::dream::evolution::{
    capture_episodes, update_environment, update_policies, value_episodes,
};
use tm_memory::{EpisodeStatus, PolicyStatus, RewardSource, TraceKind};

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

#[tokio::test]
async fn dream_policy_induction_uses_distinct_evidence_and_recomputes_gain() {
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
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:policy:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let mut episodes = Vec::new();
    for index in 0..2 {
        let idempotency_key = format!("policy-{index}");
        let turn = store
            .enqueue_turn(session.id, &idempotency_key, "inspect")
            .await
            .unwrap();
        let (episode, _) = store
            .upsert_evolution_episode(tm_memory::NewEvolutionEpisodeRecord {
                session_id: session.id,
                turn_id: turn.id,
                owner_subject: "brian".to_string(),
                memory_scope: "global".to_string(),
            })
            .await
            .unwrap();
        let traces = store
            .replace_experience_traces(
                episode.id,
                vec![
                    tm_memory::NewExperienceTraceRecord {
                        episode_id: episode.id,
                        ordinal: 0,
                        kind: TraceKind::Effect,
                        capability: Some("fs.read".to_string()),
                        action_summary: format!("read config {index}"),
                        observation_summary: "done".to_string(),
                        error_signature: None,
                        event_seq: 1,
                        result_event_seq: Some(2),
                    },
                    tm_memory::NewExperienceTraceRecord {
                        episode_id: episode.id,
                        ordinal: 1,
                        kind: TraceKind::Terminal,
                        capability: None,
                        action_summary: "turn terminal".to_string(),
                        observation_summary: "done".to_string(),
                        error_signature: None,
                        event_seq: 3,
                        result_event_seq: None,
                    },
                ],
            )
            .await
            .unwrap();
        let valued = store
            .set_episode_valuation(
                episode.id,
                0.5,
                RewardSource::Runtime,
                None,
                &[(traces[0].id, 0.5), (traces[1].id, 0.1)],
                EpisodeStatus::Valued,
            )
            .await
            .unwrap();
        episodes.push(valued);
    }
    let sink = StoreCodingEventSink::new(
        session.id,
        Arc::clone(&store),
        test_sender_factory()(session.id),
    );
    let config = EvolutionDreamConfig::default();

    let updated = update_policies(&store, &config, &dream, &episodes, &sink)
        .await
        .unwrap();
    assert_eq!(updated.len(), 1);
    let policy = &updated[0];
    assert_eq!(policy.signature, "fs|ok");
    assert_eq!(policy.status, PolicyStatus::Active);
    assert_eq!(policy.support_episode_ids.len(), 2);
    assert_eq!(policy.version, 1);
    assert_eq!(policy.procedure.lines().count(), 2);
    assert!((policy.gain - 0.11428571).abs() < 0.000_001);
    assert!(
        policy
            .boundary
            .contains("memory scope global and the fs.* capability family")
    );
    assert_eq!(store.policy_trace_values(policy.id).await.unwrap().len(), 2);

    let rerun = update_policies(&store, &config, &dream, &episodes, &sink)
        .await
        .unwrap();
    assert_eq!(rerun.len(), 1);
    assert_eq!(rerun[0].id, policy.id);
    assert_eq!(rerun[0].version, 1);
    assert_eq!(store.policy_trace_values(policy.id).await.unwrap().len(), 2);
    let progress = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("evolution_policies_updated")
        })
        .collect::<Vec<_>>();
    assert_eq!(progress[0].payload_json["policies"], json!(1));
    assert_eq!(progress[0].payload_json["induced"], json!(1));
    assert_eq!(progress[1].payload_json["induced"], json!(0));
}

#[tokio::test]
async fn dream_environment_cognition_is_project_scoped_declarative_and_versioned() {
    let store = Arc::new(InMemoryStore::default());
    store
        .ensure_project("tempestmiku", "TempestMiku", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("serious_engineer"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "project:tempestmiku".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:environment:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    let now = Utc::now();
    let fs_policy = store
        .upsert_evolution_policy(tm_memory::EvolutionPolicyRecord {
            id: Uuid::new_v4(),
            owner_subject: "brian".to_string(),
            memory_scope: "project:tempestmiku".to_string(),
            signature: "fs|enoent".to_string(),
            trigger: "Read project configuration safely".to_string(),
            procedure: "- inspect the requested path".to_string(),
            verification: "The file is read or the missing path is reported.".to_string(),
            boundary: "Project-scoped fs work only.".to_string(),
            support_episode_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            gain: 0.3,
            status: PolicyStatus::Active,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    let code_policy = store
        .upsert_evolution_policy(tm_memory::EvolutionPolicyRecord {
            id: Uuid::new_v4(),
            owner_subject: "brian".to_string(),
            memory_scope: "project:tempestmiku".to_string(),
            signature: "code|ok".to_string(),
            trigger: "Inspect symbols before editing".to_string(),
            procedure: "- inspect symbols".to_string(),
            verification: "The requested symbols are resolved.".to_string(),
            boundary: "Project-scoped code inspection only.".to_string(),
            support_episode_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            gain: 0.2,
            status: PolicyStatus::Active,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    let sink = StoreCodingEventSink::new(
        session.id,
        Arc::clone(&store),
        test_sender_factory()(session.id),
    );
    let config = EvolutionDreamConfig::default();

    let cognition = update_environment(&store, &config, &dream, &sink)
        .await
        .unwrap()
        .expect("two active policies induce project cognition");
    assert_eq!(
        cognition.title,
        "Environment cognition for project:tempestmiku"
    );
    assert_eq!(
        cognition.body,
        "Capability families in active use: code, fs.\n\
Recurring failure families: enoent.\n\
Established procedures exist for: Inspect symbols before editing.\n\
Established procedures exist for: Read project configuration safely."
    );
    assert_eq!(
        cognition.source_policy_ids,
        vec![code_policy.id, fs_policy.id]
    );
    assert!((cognition.confidence - 0.4).abs() < f32::EPSILON);
    assert_eq!(cognition.version, 1);

    let unchanged = update_environment(&store, &config, &dream, &sink)
        .await
        .unwrap()
        .expect("same active policies retain cognition");
    assert_eq!(unchanged.id, cognition.id);
    assert_eq!(unchanged.version, 1);

    let gated = update_environment(
        &store,
        &EvolutionDreamConfig {
            l3_min_policies: 3,
            ..config.clone()
        },
        &dream,
        &sink,
    )
    .await
    .unwrap();
    assert!(gated.is_none());
    assert_eq!(
        store
            .environment_cognition("brian", "project:tempestmiku")
            .await
            .unwrap()
            .expect("existing cognition remains")
            .version,
        1
    );

    let mut changed_policy = fs_policy;
    changed_policy.trigger = "Read project configuration with path checks".to_string();
    store.upsert_evolution_policy(changed_policy).await.unwrap();
    let changed = update_environment(&store, &config, &dream, &sink)
        .await
        .unwrap()
        .expect("changed policy updates cognition");
    assert_eq!(changed.id, cognition.id);
    assert_eq!(changed.version, 2);
    assert!(changed.body.contains("with path checks"));

    let progress = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("evolution_environment_updated")
        })
        .collect::<Vec<_>>();
    assert_eq!(progress.len(), 3);
    assert!(
        progress
            .iter()
            .all(|event| event.payload_json["policies"] == json!(2))
    );

    let global_dream = DreamQueueRecord {
        scope: "global".to_string(),
        ..dream
    };
    assert!(
        update_environment(&store, &config, &global_dream, &sink)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn dream_environment_persistence_failure_enqueues_and_completes_retry() {
    let store = Arc::new(InMemoryStore::default());
    store
        .ensure_project("tempestmiku", "TempestMiku", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("serious_engineer"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    store
        .set_session_memory_context(
            session.id,
            Some("tempestmiku"),
            crate::MemoryPolicy::Project,
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let now = Utc::now();
    for (signature, trigger) in [
        ("fs|enoent", "Read project configuration safely"),
        ("code|ok", "Inspect symbols before editing"),
    ] {
        store
            .upsert_evolution_policy(tm_memory::EvolutionPolicyRecord {
                id: Uuid::new_v4(),
                owner_subject: "brian".to_string(),
                memory_scope: "project:tempestmiku".to_string(),
                signature: signature.to_string(),
                trigger: trigger.to_string(),
                procedure: "- inspect current project evidence".to_string(),
                verification: "The operation completes with a checked result.".to_string(),
                boundary: "Project-scoped work only.".to_string(),
                support_episode_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
                gain: 0.2,
                status: PolicyStatus::Active,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
    }
    let original = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "project:tempestmiku".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:environment-retry:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap();
    store.fail_next_upsert_environment_cognition();
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
    assert!(
        store
            .environment_cognition("brian", "project:tempestmiku")
            .await
            .unwrap()
            .is_none()
    );
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].id, original.id);
    assert_eq!(dreams[0].status, DreamStatus::Completed);
    assert_eq!(dreams[1].status, DreamStatus::Queued);
    assert_eq!(
        dreams[1].dedupe_key,
        format!("dream:evolution-environment:{}:{}", session.id, original.id)
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
        .expect("cognition persistence failure is replayable");
    assert!(
        skipped.payload_json["reason"]
            .as_str()
            .unwrap()
            .contains("simulated environment cognition persistence failure")
    );

    let first_retry_id = dreams[1].id;
    store.fail_next_upsert_environment_cognition();

    let second = worker.run_once_result().await.unwrap();
    assert_eq!(second.completed, 1);
    assert!(
        store
            .environment_cognition("brian", "project:tempestmiku")
            .await
            .unwrap()
            .is_none()
    );
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[1].id, first_retry_id);
    assert_eq!(dreams[1].status, DreamStatus::Completed);
    assert_eq!(dreams[2].status, DreamStatus::Queued);
    assert_eq!(
        dreams[2].dedupe_key,
        format!(
            "dream:evolution-environment:{}:{}",
            session.id, first_retry_id
        )
    );

    let third = worker.run_once_result().await.unwrap();
    assert_eq!(third.completed, 1);
    assert!(
        store
            .environment_cognition("brian", "project:tempestmiku")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store.dream_queue_for_session(session.id).await.unwrap()[2].status,
        DreamStatus::Completed
    );
}
