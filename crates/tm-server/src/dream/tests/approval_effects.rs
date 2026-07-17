use super::support::*;
use super::*;

#[tokio::test]
async fn approval_effect_publish_failure_does_not_requeue_committed_mutation() {
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
    let now = Utc::now();
    let proposal = crate::memory::MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "persist this exactly once".to_string(),
        "fault-test".to_string(),
        "fault-test".to_string(),
        json!({"source": "fault-test"}),
        now,
    )
    .unwrap();
    let proposal_id = proposal.proposal_id;
    let evolution = crate::evolution::evolution_effect_metadata(
        tm_host::SelfEvolutionTier::Conservative,
        crate::evolution::memory_target_class(proposal.memory_kind),
        proposal_id.to_string(),
        "test-worker",
        session.id,
        None,
        &proposal,
    )
    .unwrap();
    let approval_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "fault-test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({"scope": "global"}),
            options_json: json!([]),
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposal": proposal,
            }),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "approved".to_string(),
                selected_option_id: Some("allow".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "approved",
                    "outcome": "approved",
                }),
                resolved_at: now + Duration::seconds(1),
            },
        )
        .await
        .unwrap();
    let lease = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            now + Duration::seconds(1),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("resolved memory effect");
    let approval = store
        .approval_request(session.id, approval_id)
        .await
        .unwrap();

    let error = crate::api::approvals::apply_approval_effect_lease(
        store.as_ref(),
        &approval,
        &lease,
        tm_host::SelfEvolutionTier::Conservative,
        &tm_modes::ModesConfig::default(),
        Arc::new(FailingPublishSink),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("post-commit broadcast failure"));
    assert!(
        store
            .claim_approval_effect(
                approval_id,
                Uuid::new_v4(),
                now + Duration::minutes(2),
                Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none(),
        "a broadcast failure must not requeue an applied effect"
    );
    assert_eq!(
        store.recall_chunks("global", "", 10).await.unwrap().len(),
        1
    );
    assert_eq!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .filter(|event| {
                event.event_type == "write_proposal"
                    && event.payload_json["proposalId"] == json!(proposal_id)
                    && event.payload_json["status"] == json!("approved")
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn restart_with_lower_tier_blocks_queued_approved_memory_effect() {
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
    let proposal = crate::MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "must not survive a tier downgrade".to_string(),
        "tier-test".to_string(),
        "tier-test".to_string(),
        json!({"source": "tier-test"}),
        Utc::now(),
    )
    .unwrap();
    let metadata = crate::evolution::evolution_effect_metadata(
        tm_host::SelfEvolutionTier::Conservative,
        crate::evolution::memory_target_class(proposal.memory_kind),
        proposal.proposal_id.to_string(),
        "test-worker",
        session.id,
        None,
        &proposal,
    )
    .unwrap();
    let (approval, lease) = resolved_memory_effect(
        store.as_ref(),
        session.id,
        proposal,
        serde_json::to_value(metadata).unwrap(),
    )
    .await;

    let memory = Arc::new(crate::StoreMemoryProvider::new(Arc::clone(&store)));
    let restarted = crate::AppState::new(
        Arc::clone(&store),
        memory,
        Arc::new(crate::EchoChatRunner),
        ModesConfig::default(),
        crate::AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Off);

    let error = crate::api::approvals::apply_approval_effect_lease(
        restarted.store.as_ref(),
        &approval,
        &lease,
        restarted.self_evolution_tier,
        &restarted.persona,
        Arc::new(PersistedOnlySink),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("evolution_disabled"));
    assert!(
        store
            .recall_chunks("global", "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .all(|event| {
                event.event_type != "write_proposal"
                    || event.payload_json["status"] != json!("approved")
            })
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
            tm_host::EvolutionAuditStatus::Failed,
        ]
    );
    assert!(audits.iter().all(|record| {
        record.origin.session_id == session.id
            && record.origin.actor_id.as_str() == "test-worker"
            && record.approval_id == Some(approval.id)
            && record.effect_id == Some(approval.id)
            && record.content_digest.as_str().starts_with("sha256:")
    }));
    assert_eq!(
        audits.last().and_then(|record| record.error_code),
        Some(tm_host::EvolutionPolicyReason::DisabledTier)
    );
}

#[tokio::test]
async fn approved_memory_effect_rejects_forged_target_metadata() {
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
    let proposal = crate::MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "forged metadata must not write".to_string(),
        "forged-test".to_string(),
        "forged-test".to_string(),
        json!({"source": "forged-test"}),
        Utc::now(),
    )
    .unwrap();
    let forged = crate::evolution::evolution_effect_metadata(
        tm_host::SelfEvolutionTier::Conservative,
        tm_host::EvolutionTargetClass::SkillProposal,
        proposal.proposal_id.to_string(),
        "test-worker",
        session.id,
        None,
        &proposal,
    )
    .unwrap();
    let (approval, lease) = resolved_memory_effect(
        &store,
        session.id,
        proposal,
        serde_json::to_value(forged).unwrap(),
    )
    .await;

    let error = crate::api::approvals::apply_approval_effect_lease(
        &store,
        &approval,
        &lease,
        tm_host::SelfEvolutionTier::Conservative,
        &tm_modes::ModesConfig::default(),
        Arc::new(PersistedOnlySink),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("evolution_invalid_payload"));
    assert!(
        store
            .recall_chunks("global", "", 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_approved_memory_effect_is_fenced_before_mutation() {
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
    let proposal = crate::MemoryWriteProposal::recall_chunk(
        "brian".to_string(),
        "global".to_string(),
        "stale workers must not write".to_string(),
        "stale-test".to_string(),
        "stale-test".to_string(),
        json!({"source": "stale-test"}),
        Utc::now(),
    )
    .unwrap();
    let metadata = crate::evolution::evolution_effect_metadata(
        tm_host::SelfEvolutionTier::Conservative,
        crate::evolution::memory_target_class(proposal.memory_kind),
        proposal.proposal_id.to_string(),
        "test-worker",
        session.id,
        None,
        &proposal,
    )
    .unwrap();
    let (approval, stale_lease) = resolved_memory_effect(
        &store,
        session.id,
        proposal,
        serde_json::to_value(metadata).unwrap(),
    )
    .await;
    let replacement = store
        .claim_approval_effect(
            approval.id,
            Uuid::new_v4(),
            Utc::now() + Duration::minutes(10),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("stale effect is reclaimed");
    assert!(replacement.epoch > stale_lease.epoch);

    let error = crate::api::approvals::apply_approval_effect_lease(
        &store,
        &approval,
        &stale_lease,
        tm_host::SelfEvolutionTier::Conservative,
        &tm_modes::ModesConfig::default(),
        Arc::new(PersistedOnlySink),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("evolution_stale_approval"));
    assert!(
        store
            .recall_chunks("global", "", 10)
            .await
            .unwrap()
            .is_empty()
    );
}
