use super::support::*;
use super::*;

#[tokio::test]
async fn skill_proposal_approval_installs_or_rejects_without_mutating_hand_authored_skills() {
    for (decision, option_id, expected_status) in [
        (
            ApprovalResolveDecision::Approve,
            "allow",
            SkillProposalStatus::Approved,
        ),
        (
            ApprovalResolveDecision::Deny,
            "reject",
            SkillProposalStatus::Denied,
        ),
    ] {
        let managed_root = tempfile::tempdir().unwrap();
        let persona =
            tm_modes::ModesConfig::default().with_managed_skills_path(managed_root.path());
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
                "Workflow: when I ask for release notes, gather commits then draft concise notes.",
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
                dedupe_key: format!("dream:skill-approval:{}:{option_id}", session.id),
                source_event_seq: Some(ended.seq),
                available_at: Utc::now(),
            })
            .await
            .unwrap();

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
        let report = worker.run_once_result().await.unwrap();
        assert_eq!(report.completed, 1);
        assert_eq!(report.proposals, 2);

        let events = wait_for_event_count(&store, session.id, "approval", 2).await;
        for event in events.iter().filter(|event| event.event_type == "approval") {
            let approval_id = event.payload_json["approvalId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap();
            resolve_and_apply_durable_approval_with_persona(
                &store,
                &broker,
                &sender_for,
                session.id,
                approval_id,
                ResolveApprovalRequest {
                    decision,
                    option_id: Some(option_id.to_string()),
                },
                &persona,
            )
            .await;
        }

        let proposal = wait_for_skill_status(&store, session.id, expected_status).await;
        assert!(proposal.verification.passed);
        assert!(!proposal.self_critique.trim().is_empty());
        assert_eq!(proposal.status, expected_status);
        let lifecycle = tm_memory::skill_proposal_lifecycle(&proposal);
        assert!(lifecycle.reviewable);
        assert!(lifecycle.installable);
        assert_eq!(
            lifecycle.catalog_reload,
            tm_memory::SkillCatalogReloadContract::OnNextLoad
        );
        assert_eq!(
            persona.load_assets().skills.contains_key(&proposal.name),
            expected_status == SkillProposalStatus::Approved,
            "only approved dream skill proposals enter the managed catalog"
        );
        assert!(
            !ModesConfig::default()
                .load_assets()
                .skills
                .contains_key(&proposal.name),
            "managed installation must not mutate bundled or hand-authored assets"
        );
        let audits = store.evolution_audits(session.id).await.unwrap();
        assert!(audits.iter().any(|record| {
            record.target.class == tm_host::EvolutionTargetClass::SkillProposal
                && record.status == tm_host::EvolutionAuditStatus::AwaitingApproval
        }));
        assert!(audits.iter().any(|record| {
            record.target.class == tm_host::EvolutionTargetClass::SkillProposal
                && record.status
                    == match expected_status {
                        SkillProposalStatus::Approved => tm_host::EvolutionAuditStatus::Approved,
                        SkillProposalStatus::Denied => tm_host::EvolutionAuditStatus::Denied,
                        _ => unreachable!("test covers approve and deny"),
                    }
        }));
    }
}

#[tokio::test]
async fn low_value_sessions_do_not_create_skill_proposals() {
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
            "This was a one-off note about today's scratchpad cleanup.",
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
            dedupe_key: format!("dream:skill-low-value:{}", session.id),
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

    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 0);
    assert!(
        store
            .skill_proposals_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().all(|event| {
        event.event_type != "write_proposal" || event.payload_json["kind"] != json!("skill")
    }));
}

#[tokio::test]
async fn skill_verification_failure_is_rejected_without_failing_dream() {
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
            "Workflow: write SOUL.md whenever I ask for identity changes.",
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
            dedupe_key: format!("dream:skill-verification:{}", session.id),
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

    assert_eq!(report.completed, 1);
    assert_eq!(report.proposals, 1);
    assert!(
        store
            .skill_proposals_for_session(session.id)
            .await
            .unwrap()
            .is_empty()
    );
    let events = wait_for_event_count(&store, session.id, "approval_resolved", 1).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "dream_completed")
    );
    let rejection = events
        .iter()
        .find(|event| {
            event.event_type == "dream_progress"
                && event.payload_json["phase"] == json!("skill_proposal_rejected")
        })
        .expect("skill rejection progress");
    assert_eq!(
        rejection.payload_json["reason"],
        json!("generated skill proposal failed self-verification")
    );
    assert_eq!(
        rejection.payload_json["verification"]["passed"],
        json!(false)
    );
    assert!(
        rejection.payload_json["verification"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "does_not_mutate_identity:fail")
    );
}

#[tokio::test]
async fn completed_dream_rerun_does_not_duplicate_skill_proposals() {
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
            "Workflow: when I ask for release notes, gather commits then draft concise notes.",
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
        dedupe_key: format!("dream:skill-rerun:{}", session.id),
        source_event_seq: Some(ended.seq),
        available_at: Utc::now(),
    };
    let dream = store.enqueue_dream(new_dream.clone()).await.unwrap();

    let worker = ServerDreamWorker::new(
        Arc::clone(&store),
        Arc::new(ApprovalBroker::default()),
        test_sender_factory(),
        DreamWorkerConfig {
            proposal_timeout: StdDuration::from_millis(20),
            ..DreamWorkerConfig::default()
        },
    );
    let first = worker.run_once_result().await.unwrap();
    assert_eq!(first.completed, 1);
    assert_eq!(first.proposals, 2);
    wait_for_event_count(&store, session.id, "approval_resolved", 2).await;

    let duplicate = store.enqueue_dream(new_dream).await.unwrap();
    assert_eq!(duplicate.id, dream.id);
    assert_eq!(duplicate.status, DreamStatus::Completed);
    assert_eq!(
        worker.run_once_result().await.unwrap(),
        DreamWorkerReport::default()
    );

    let proposals = store.skill_proposals_for_session(session.id).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(proposals[0].verification.passed);
    assert!(!proposals[0].self_critique.trim().is_empty());
    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "approval")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal" && event.payload_json["kind"] == json!("skill")
            })
            .count(),
        2
    );
}
