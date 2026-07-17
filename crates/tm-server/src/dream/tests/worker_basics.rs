use super::support::*;
use super::*;

#[test]
fn dream_worker_config_defaults_cover_p4_knobs() {
    let config = DreamWorkerConfig::default();

    assert!(config.enabled);
    assert!(config.redaction.enabled);
    assert!(config.summary_cadence.session_every_dream);
    assert!(config.summary_cadence.rollup_every_dream);
    assert_eq!(config.retry_backoff, Duration::seconds(30));
    assert_eq!(config.input_budget, DreamInputBudget::default());
    assert_eq!(config.reflect_importance_threshold, 1.5);
    assert_eq!(config.model_roles.reflection, "cheap");
}

#[tokio::test]
async fn dream_worker_store_claim_failure_is_reported_and_trait_degrades() {
    let store = Arc::new(ClaimFailureStore);
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );

    let err = worker.run_once_result().await.unwrap_err();
    assert_eq!(err.to_string(), "store error: claim failed");
    assert_eq!(worker.run_once().await, DreamWorkerReport::default());
}

#[tokio::test]
async fn dream_worker_writes_summary_and_defers_proposals() {
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
            "Workflow: when I ask for release notes, gather commits then draft concise notes. token=sk-testsecret123456",
        )
        .await
        .unwrap();
    store
        .append_message(session.id, "user", "I prefer small, reviewable patches.")
        .await
        .unwrap();
    store
        .append_message(
            session.id,
            "assistant",
            "Captured the release-note workflow and left artifact://0 for review.",
        )
        .await
        .unwrap();
    store.end_session(session.id).await.unwrap();
    let ended = store
        .append_event(
            session.id,
            "session_end",
            json!({"status": "ended", "reason": "test"}),
        )
        .await
        .unwrap();
    let dream = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::SessionEnded,
            dedupe_key: format!("dream:test:{}", session.id),
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
    let report = worker.run_once_result().await.unwrap();

    assert_eq!(report.attempted, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 2);
    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams[0].id, dream.id);
    assert_eq!(dreams[0].status, DreamStatus::Completed);

    let summaries = store.memory_summaries("global", 10).await.unwrap();
    let session_summary = summaries
        .iter()
        .find(|summary| summary.kind == MemorySummaryKind::Session)
        .expect("session summary");
    assert!(session_summary.body.contains("[REDACTED_SECRET]"));
    assert!(!session_summary.body.contains("sk-testsecret123456"));
    assert_eq!(session_summary.source_dream_id, dream.id);
    assert!(
        session_summary
            .evidence
            .iter()
            .any(|evidence| evidence.event_seq == Some(ended.seq))
    );

    let proposals = store.skill_proposals_for_session(session.id).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(proposals[0].verification.passed);
    assert!(matches!(
        proposals[0].status,
        SkillProposalStatus::Pending | SkillProposalStatus::TimedOut
    ));

    let events = wait_for_event_count(&store, session.id, "approval_resolved", 2).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_started")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_completed")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "write_proposal")
            .count(),
        4
    );
    assert_eq!(
        store.profile_facts("brian").await.unwrap(),
        Vec::new(),
        "timed-out dream memory approval must not write profile facts"
    );
    assert_eq!(
        store.skill_proposal(proposals[0].id).await.unwrap().status,
        SkillProposalStatus::TimedOut
    );
}

#[tokio::test]
async fn concurrent_dream_workers_do_not_complete_same_dream_twice() {
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
        .append_message(session.id, "assistant", "Done and ready for review.")
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
            dedupe_key: format!("dream:race:{}", session.id),
            source_event_seq: Some(ended.seq),
            available_at: Utc::now(),
        })
        .await
        .unwrap();

    let broker = Arc::new(ApprovalBroker::default());
    let worker_a = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::clone(&broker),
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );
    let worker_b = ServerDreamWorker::new(
        Arc::clone(&store),
        broker,
        test_sender_factory(),
        DreamWorkerConfig::default(),
    );

    let (first, second) = tokio::join!(worker_a.run_once_result(), worker_b.run_once_result());
    let reports = [first.unwrap(), second.unwrap()];
    assert_eq!(
        reports.iter().map(|report| report.attempted).sum::<usize>(),
        1
    );
    assert_eq!(
        reports.iter().map(|report| report.completed).sum::<usize>(),
        1
    );
    assert_eq!(
        reports.iter().map(|report| report.proposals).sum::<usize>(),
        0
    );

    let dreams = store.dream_queue_for_session(session.id).await.unwrap();
    assert_eq!(dreams.len(), 1);
    assert_eq!(dreams[0].status, DreamStatus::Completed);
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::Session)
            .count(),
        1
    );
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::TopicProject)
            .count(),
        1
    );

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_started")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "dream_completed")
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "dream_failed")
    );
}

#[tokio::test]
async fn completed_dream_rerun_does_not_duplicate_memory_or_approvals() {
    let store = Arc::new(InMemoryStore::default());
    let broker = Arc::new(ApprovalBroker::default());
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
            "Remember that I prefer deterministic dream reruns.",
        )
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
        dedupe_key: format!("dream:rerun:{}", session.id),
        source_event_seq: Some(ended.seq),
        available_at: Utc::now(),
    };
    let dream = store.enqueue_dream(new_dream.clone()).await.unwrap();

    let sender_for = test_sender_factory();
    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::clone(&broker),
        Arc::clone(&sender_for),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_secs(5),
            ..DreamWorkerConfig::default()
        },
    );
    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.attempted, 1);
    assert_eq!(first.completed, 1);
    assert_eq!(first.proposals, 1);

    let events = wait_for_event_count(&store, session.id, "approval", 1).await;
    let approval_id = events
        .iter()
        .find(|event| event.event_type == "approval")
        .unwrap()
        .payload_json["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_and_apply_durable_approval(
        &store,
        &broker,
        &sender_for,
        session.id,
        approval_id,
        ResolveApprovalRequest {
            decision: ApprovalResolveDecision::Approve,
            option_id: Some("allow".to_string()),
        },
    )
    .await;
    wait_for_memory_write_status(&store, session.id, MemoryWriteStatus::Approved).await;

    let duplicate = store.enqueue_dream(new_dream).await.unwrap();
    assert_eq!(duplicate.id, dream.id);
    assert_eq!(duplicate.status, DreamStatus::Completed);
    let second = worker.run_once_result().await.unwrap();
    assert_eq!(second, DreamWorkerReport::default());

    let facts = store.profile_facts("brian").await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object, "deterministic dream reruns");
    let summaries = store.memory_summaries("global", 10).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::Session)
            .count(),
        1
    );
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.kind == MemorySummaryKind::TopicProject)
            .count(),
        1
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "approval")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["status"] == json!("approved")
            })
            .count(),
        1
    );
    let audits = store.evolution_audits(session.id).await.unwrap();
    assert_eq!(audits.len(), 3);
    assert_eq!(
        audits
            .iter()
            .map(|record| record.status)
            .collect::<Vec<_>>(),
        vec![
            tm_host::EvolutionAuditStatus::AwaitingApproval,
            tm_host::EvolutionAuditStatus::Approved,
            tm_host::EvolutionAuditStatus::Applied,
        ]
    );
}
