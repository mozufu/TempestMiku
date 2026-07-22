use super::*;
use tm_memory::{
    EpisodeStatus, FeedbackOutcome, NewEvolutionEpisodeRecord, NewExperienceTraceRecord,
    RewardSource, TraceKind,
};

#[tokio::test]
async fn evolution_episode_trace_and_feedback_lifecycle_is_atomic_and_idempotent() {
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
    let turn = store
        .enqueue_turn(session.id, "evolution-turn", "inspect the project")
        .await
        .unwrap();

    let (episode, inserted) = store
        .upsert_evolution_episode(NewEvolutionEpisodeRecord {
            session_id: session.id,
            turn_id: turn.id,
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
        })
        .await
        .unwrap();
    assert!(inserted);
    let (duplicate, inserted) = store
        .upsert_evolution_episode(NewEvolutionEpisodeRecord {
            session_id: session.id,
            turn_id: turn.id,
            owner_subject: "ignored".to_string(),
            memory_scope: "project:ignored".to_string(),
        })
        .await
        .unwrap();
    assert!(!inserted);
    assert_eq!(duplicate, episode);

    let traces = store
        .replace_experience_traces(
            episode.id,
            vec![
                NewExperienceTraceRecord {
                    episode_id: episode.id,
                    ordinal: 1,
                    kind: TraceKind::Effect,
                    capability: Some("fs.read".to_string()),
                    action_summary: "read configuration".to_string(),
                    observation_summary: "completed".to_string(),
                    error_signature: None,
                    event_seq: 2,
                    result_event_seq: Some(3),
                },
                NewExperienceTraceRecord {
                    episode_id: episode.id,
                    ordinal: 0,
                    kind: TraceKind::Cell,
                    capability: None,
                    action_summary: "execute cell".to_string(),
                    observation_summary: "completed".to_string(),
                    error_signature: None,
                    event_seq: 1,
                    result_event_seq: Some(4),
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(traces.len(), 2);
    let persisted = store.experience_traces(episode.id).await.unwrap();
    assert_eq!(persisted[0].ordinal, 0);
    assert_eq!(persisted[1].ordinal, 1);
    assert_eq!(
        store
            .evolution_episode(episode.id)
            .await
            .unwrap()
            .trace_count,
        2
    );

    let values = persisted
        .iter()
        .map(|trace| (trace.id, if trace.ordinal == 0 { 0.4 } else { 0.5 }))
        .collect::<Vec<_>>();
    let valued = store
        .set_episode_valuation(
            episode.id,
            0.5,
            RewardSource::Runtime,
            None,
            &values,
            EpisodeStatus::Valued,
        )
        .await
        .unwrap();
    assert_eq!(valued.status, EpisodeStatus::Valued);
    assert_eq!(valued.terminal_reward, Some(0.5));
    assert_eq!(
        store
            .experience_traces(episode.id)
            .await
            .unwrap()
            .iter()
            .map(|trace| trace.value)
            .collect::<Vec<_>>(),
        vec![Some(0.4), Some(0.5)]
    );

    assert!(
        store
            .record_turn_feedback(
                session.id,
                turn.id,
                FeedbackOutcome::Accepted,
                Some("provider returned sk-testsecret123456"),
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .record_turn_feedback(
                session.id,
                turn.id,
                FeedbackOutcome::Accepted,
                Some("different comment is ignored"),
            )
            .await
            .unwrap()
    );
    let feedback = store.turn_feedback(turn.id).await.unwrap().unwrap();
    assert_eq!(feedback.0, FeedbackOutcome::Accepted);
    assert!(!feedback.1.unwrap().contains("sk-testsecret123456"));
    assert!(matches!(
        store
            .record_turn_feedback(
                session.id,
                turn.id,
                FeedbackOutcome::Rejected,
                None,
            )
            .await,
        Err(ServerError::Conflict(message)) if message == "turn feedback already recorded"
    ));

    let listed = store
        .evolution_episodes("brian", "global", 10)
        .await
        .unwrap();
    assert_eq!(listed, vec![valued]);
}

#[tokio::test]
async fn trace_replacement_rejects_cross_episode_and_valuation_rolls_back_on_unknown_trace() {
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
    let turn = store
        .enqueue_turn(session.id, "evolution-boundary", "inspect")
        .await
        .unwrap();
    let (episode, _) = store
        .upsert_evolution_episode(NewEvolutionEpisodeRecord {
            session_id: session.id,
            turn_id: turn.id,
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
        })
        .await
        .unwrap();

    assert!(matches!(
        store
            .replace_experience_traces(
                episode.id,
                vec![NewExperienceTraceRecord {
                    episode_id: Uuid::new_v4(),
                    ordinal: 0,
                    kind: TraceKind::Terminal,
                    capability: None,
                    action_summary: "terminal".to_string(),
                    observation_summary: "done".to_string(),
                    error_signature: None,
                    event_seq: 1,
                    result_event_seq: None,
                }],
            )
            .await,
        Err(ServerError::InvalidRequest(_))
    ));

    assert!(matches!(
        store
            .replace_experience_traces(
                episode.id,
                vec![
                    NewExperienceTraceRecord {
                        episode_id: episode.id,
                        ordinal: 0,
                        kind: TraceKind::Cell,
                        capability: None,
                        action_summary: "first".to_string(),
                        observation_summary: "done".to_string(),
                        error_signature: None,
                        event_seq: 1,
                        result_event_seq: None,
                    },
                    NewExperienceTraceRecord {
                        episode_id: episode.id,
                        ordinal: 0,
                        kind: TraceKind::Terminal,
                        capability: None,
                        action_summary: "duplicate".to_string(),
                        observation_summary: "done".to_string(),
                        error_signature: None,
                        event_seq: 2,
                        result_event_seq: None,
                    },
                ],
            )
            .await,
        Err(ServerError::InvalidRequest(message))
            if message == "experience trace ordinals must be unique"
    ));

    let traces = store
        .replace_experience_traces(
            episode.id,
            vec![NewExperienceTraceRecord {
                episode_id: episode.id,
                ordinal: 0,
                kind: TraceKind::Terminal,
                capability: None,
                action_summary: "terminal".to_string(),
                observation_summary: "done".to_string(),
                error_signature: None,
                event_seq: 1,
                result_event_seq: None,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .set_episode_valuation(
                episode.id,
                1.0,
                RewardSource::Explicit,
                Some(FeedbackOutcome::Accepted),
                &[(Uuid::new_v4(), 1.0)],
                EpisodeStatus::Valued,
            )
            .await,
        Err(ServerError::NotFound(_))
    ));
    assert_eq!(
        store.experience_traces(episode.id).await.unwrap()[0].value,
        None
    );
    assert_eq!(traces.len(), 1);
}
