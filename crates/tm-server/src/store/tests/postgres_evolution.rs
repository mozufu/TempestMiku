use super::*;
use tm_memory::{
    EpisodeStatus, EvolutionPolicyRecord, FeedbackOutcome, NewEvolutionEpisodeRecord,
    NewExperienceTraceRecord, PolicyStatus, RewardSource, TraceKind,
};

#[tokio::test]
async fn gated_postgres_evolution_valuation_is_atomic_and_survives_restart() {
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
    let turn = store
        .enqueue_turn(
            session.id,
            &format!("evolution-valuation-{}", Uuid::new_v4()),
            "inspect",
        )
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
    let traces = store
        .replace_experience_traces(
            episode.id,
            vec![
                NewExperienceTraceRecord {
                    episode_id: episode.id,
                    ordinal: 0,
                    kind: TraceKind::Effect,
                    capability: Some("fs.read".to_string()),
                    action_summary: "read config".to_string(),
                    observation_summary: "done".to_string(),
                    error_signature: None,
                    event_seq: 1,
                    result_event_seq: Some(2),
                },
                NewExperienceTraceRecord {
                    episode_id: episode.id,
                    ordinal: 1,
                    kind: TraceKind::Terminal,
                    capability: None,
                    action_summary: "final".to_string(),
                    observation_summary: "done".to_string(),
                    error_signature: None,
                    event_seq: 3,
                    result_event_seq: None,
                },
            ],
        )
        .await
        .unwrap();
    let foreign_turn = store
        .enqueue_turn(
            session.id,
            &format!("evolution-foreign-{}", Uuid::new_v4()),
            "inspect",
        )
        .await
        .unwrap();
    let (foreign_episode, _) = store
        .upsert_evolution_episode(NewEvolutionEpisodeRecord {
            session_id: session.id,
            turn_id: foreign_turn.id,
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
        })
        .await
        .unwrap();
    let foreign_trace = store
        .replace_experience_traces(
            foreign_episode.id,
            vec![NewExperienceTraceRecord {
                episode_id: foreign_episode.id,
                ordinal: 0,
                kind: TraceKind::Terminal,
                capability: None,
                action_summary: "final".to_string(),
                observation_summary: "done".to_string(),
                error_signature: None,
                event_seq: 4,
                result_event_seq: None,
            }],
        )
        .await
        .unwrap()
        .remove(0);

    let error = store
        .set_episode_valuation(
            episode.id,
            1.0,
            RewardSource::Explicit,
            Some(FeedbackOutcome::Accepted),
            &[(traces[0].id, 0.9), (foreign_trace.id, 1.0)],
            &[],
            EpisodeStatus::Valued,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ServerError::NotFound(_)));
    assert!(
        store
            .experience_traces(episode.id)
            .await
            .unwrap()
            .iter()
            .all(|trace| trace.value.is_none())
    );
    assert_eq!(
        store.evolution_episode(episode.id).await.unwrap().status,
        EpisodeStatus::Captured
    );

    store
        .set_episode_valuation(
            episode.id,
            -0.4,
            RewardSource::Explicit,
            Some(FeedbackOutcome::Corrected),
            &[(traces[0].id, -0.37), (traces[1].id, -0.4)],
            &[],
            EpisodeStatus::Valued,
        )
        .await
        .unwrap();
    let skill_outcome = (
        format!("release-workflow-{}", Uuid::new_v4()),
        "sha256:test".to_string(),
        false,
    );
    let conflicting = store
        .set_episode_valuation(
            episode.id,
            -0.4,
            RewardSource::Explicit,
            Some(FeedbackOutcome::Corrected),
            &[(traces[0].id, -0.37), (traces[1].id, -0.4)],
            std::slice::from_ref(&skill_outcome),
            EpisodeStatus::Valued,
        )
        .await
        .unwrap_err();
    assert!(matches!(conflicting, ServerError::Conflict(_)));
    store
        .set_episode_valuation(
            episode.id,
            -0.4,
            RewardSource::Explicit,
            Some(FeedbackOutcome::Corrected),
            &[(traces[0].id, -0.37), (traces[1].id, -0.4)],
            &[],
            EpisodeStatus::Valued,
        )
        .await
        .unwrap();
    assert!(
        store
            .skill_runtime_stats(std::slice::from_ref(&skill_outcome.0))
            .await
            .unwrap()
            .is_empty()
    );
    drop(store);

    let restarted = PostgresStore::connect(&dsn).await.unwrap();
    let valued = restarted.evolution_episode(episode.id).await.unwrap();
    assert_eq!(valued.status, EpisodeStatus::Valued);
    assert_eq!(valued.terminal_reward, Some(-0.4));
    assert_eq!(valued.reward_source, Some(RewardSource::Explicit));
    assert_eq!(valued.feedback_outcome, Some(FeedbackOutcome::Corrected));
    assert_eq!(
        restarted
            .experience_traces(episode.id)
            .await
            .unwrap()
            .iter()
            .map(|trace| trace.value)
            .collect::<Vec<_>>(),
        vec![Some(-0.37), Some(-0.4)]
    );
}

#[tokio::test]
async fn gated_postgres_evolution_policy_links_survive_restart() {
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
    let turn = store
        .enqueue_turn(
            session.id,
            &format!("evolution-policy-{}", Uuid::new_v4()),
            "inspect",
        )
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
    let trace = store
        .replace_experience_traces(
            episode.id,
            vec![NewExperienceTraceRecord {
                episode_id: episode.id,
                ordinal: 0,
                kind: TraceKind::Effect,
                capability: Some("fs.read".to_string()),
                action_summary: "read config".to_string(),
                observation_summary: "done".to_string(),
                error_signature: None,
                event_seq: 1,
                result_event_seq: Some(2),
            }],
        )
        .await
        .unwrap()
        .remove(0);
    store
        .set_episode_valuation(
            episode.id,
            0.5,
            RewardSource::Runtime,
            None,
            &[(trace.id, 0.5)],
            &[],
            EpisodeStatus::Valued,
        )
        .await
        .unwrap();
    let now = Utc::now();
    let policy = store
        .upsert_evolution_policy(EvolutionPolicyRecord {
            id: Uuid::new_v4(),
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            signature: format!("fs|ok|{}", Uuid::new_v4()),
            trigger: "Recurring fs work".to_string(),
            procedure: "- read config → done".to_string(),
            verification: "Verify completion".to_string(),
            boundary: "Only fs.*".to_string(),
            support_episode_ids: vec![episode.id],
            gain: 0.25,
            status: PolicyStatus::Active,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    let link = (trace.id, episode.id, 0.5, true);
    store.link_policy_traces(policy.id, &[link]).await.unwrap();
    store.link_policy_traces(policy.id, &[link]).await.unwrap();
    drop(store);

    let restarted = PostgresStore::connect(&dsn).await.unwrap();
    assert_eq!(restarted.evolution_policy(policy.id).await.unwrap(), policy);
    assert_eq!(
        restarted
            .evolution_policies("brian", "global", None, usize::MAX)
            .await
            .unwrap(),
        vec![policy.clone()]
    );
    assert_eq!(
        restarted.policy_trace_values(policy.id).await.unwrap(),
        vec![link]
    );
}
