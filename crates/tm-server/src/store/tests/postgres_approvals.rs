use super::*;

#[tokio::test]
async fn gated_postgres_approval_resolution_and_effect_claims_are_cross_instance_cas() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    let other_store = PostgresStore::connect(&dsn).await.unwrap();
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
    let approval_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let evolution = crate::evolution::evolution_effect_metadata(
        tm_host::SelfEvolutionTier::Conservative,
        tm_host::EvolutionTargetClass::ScopedMemory,
        proposal_id.to_string(),
        "postgres-test",
        session.id,
        None,
        &json!({"candidate": "cross-instance durable reminder"}),
    )
    .unwrap();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "postgres-test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({"scope": "global"}),
            options_json: json!([
                {"id": "allow", "label": "Allow"},
                {"id": "deny", "label": "Deny"}
            ]),
            effect_type: "memory_write".to_string(),
            effect_payload_json: json!({
                "evolution": evolution,
                "proposalId": proposal_id,
            }),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    assert!(
        other_store
            .claim_approval_effect(approval_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_none()
    );
    let request_event = store
        .append_event(session.id, "approval", json!({"approvalId": approval_id}))
        .await
        .unwrap();
    other_store
        .link_approval_event(session.id, approval_id, "approval", request_event.seq)
        .await
        .unwrap();

    let resolution = NewApprovalResolution {
        status: "approved".to_string(),
        selected_option_id: Some("allow".to_string()),
        resolution_json: json!({
            "approvalId": approval_id,
            "status": "approved",
            "outcome": "approved"
        }),
        resolved_at: now + Duration::seconds(1),
    };
    let (left, right) = tokio::join!(
        store.resolve_approval_request_with_event(session.id, approval_id, resolution.clone()),
        other_store.resolve_approval_request_with_event(session.id, approval_id, resolution),
    );
    assert_eq!(
        left.is_ok() as usize + right.is_ok() as usize,
        1,
        "cross-instance resolution results: left={left:?}, right={right:?}"
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(ServerError::Conflict(_))) | (Err(ServerError::Conflict(_)), Ok(_))
    ));
    assert_eq!(
        other_store
            .approval_request(session.id, approval_id)
            .await
            .unwrap()
            .resolution_version,
        1
    );
    assert_eq!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "approval_resolved")
            .count(),
        1
    );

    let first_owner = Uuid::new_v4();
    let second_owner = Uuid::new_v4();
    let claim_at = now + Duration::seconds(2);
    let (left, right) = tokio::join!(
        store.claim_next_approval_effect(first_owner, claim_at, Duration::seconds(30)),
        other_store.claim_next_approval_effect(second_owner, claim_at, Duration::seconds(30)),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.is_some() as usize + right.is_some() as usize, 1);
    let first = left.or(right).expect("one effect claim");
    assert_eq!(first.effect.approval_id, approval_id);
    let record = RecallChunkRecord {
        id: proposal_id,
        scope: "global".to_string(),
        text: "cross-instance durable reminder".to_string(),
        source: "postgres-test".to_string(),
        importance: 1.0,
        created_at: claim_at,
    };
    store.upsert_recall_chunk(record.clone()).await.unwrap();
    let reclaimed = other_store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            claim_at + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("stale claim is reclaimable across instances");
    other_store.upsert_recall_chunk(record).await.unwrap();
    let terminal_payload = json!({
        "kind": "memory",
        "proposalId": proposal_id,
        "status": "approved",
    });
    assert!(
        store
            .complete_approval_effect_with_event(
                &first,
                terminal_payload.clone(),
                None,
                claim_at + Duration::seconds(31),
            )
            .await
            .is_err()
    );
    let (applied, terminal_event) = other_store
        .complete_approval_effect_with_event(
            &reclaimed,
            terminal_payload,
            None,
            claim_at + Duration::seconds(32),
        )
        .await
        .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(terminal_event.event_type, "write_proposal");
    assert_eq!(
        store
            .recall_chunks("global", "", 10)
            .await
            .unwrap()
            .iter()
            .filter(|record| record.id == proposal_id)
            .count(),
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
    let restarted = PostgresStore::connect(&dsn).await.unwrap();
    let audits = restarted.evolution_audits(session.id).await.unwrap();
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

    let expired_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: expired_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "postgres-test".to_string(),
            action: "code.run".to_string(),
            scope_json: json!({}),
            options_json: json!([]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({}),
            resumable: false,
            created_at: now - Duration::minutes(2),
            expires_at: now - Duration::minutes(1),
        })
        .await
        .unwrap();
    let expiry_events = other_store.expire_pending_approvals(now).await.unwrap();
    assert!(expiry_events.iter().any(|event| {
        event.payload_json["approvalId"] == json!(expired_id)
            && event.event_type == "approval_resolved"
    }));
    assert_eq!(
        store
            .approval_request(session.id, expired_id)
            .await
            .unwrap()
            .status,
        "timed_out"
    );
    assert!(
        store
            .claim_approval_effect(expired_id, Uuid::new_v4(), now, Duration::seconds(30))
            .await
            .unwrap()
            .is_some()
    );
}
