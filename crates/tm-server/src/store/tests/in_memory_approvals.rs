use super::*;

#[tokio::test]
async fn in_memory_approval_resolution_is_cas_and_effect_application_is_fenced() {
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
    let now = Utc::now();
    let approval_id = Uuid::new_v4();
    let requester_id = Uuid::new_v4();
    let request = NewApprovalRequest {
        id: approval_id,
        session_id: session.id,
        turn_id: None,
        requester_id,
        origin: "test".to_string(),
        action: "memory.write".to_string(),
        scope_json: json!({"subject": "brian"}),
        options_json: json!([
            {"id": "allow", "label": "Allow"},
            {"id": "deny", "label": "Deny"}
        ]),
        effect_type: "memory_write".to_string(),
        effect_payload_json: json!({"proposalId": "proposal-1"}),
        resumable: true,
        created_at: now,
        expires_at: now + Duration::minutes(5),
    };
    let created = store
        .create_approval_request(request.clone())
        .await
        .unwrap();
    assert_eq!(created.status, "pending");
    assert_eq!(
        store.create_approval_request(request).await.unwrap().id,
        approval_id
    );
    assert!(
        store
            .claim_approval_effect(approval_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_none(),
        "effects remain blocked until the approval reaches a terminal state"
    );

    let request_event = store
        .append_event(session.id, "approval", json!({"approvalId": approval_id}))
        .await
        .unwrap();
    let linked = store
        .link_approval_event(session.id, approval_id, "approval", request_event.seq)
        .await
        .unwrap();
    assert_eq!(linked.request_event_seq, Some(request_event.seq));
    assert!(matches!(
        store
            .heartbeat_approval_request(approval_id, Uuid::new_v4(), now)
            .await,
        Err(ServerError::Conflict(_))
    ));
    assert_eq!(
        store
            .heartbeat_approval_request(approval_id, requester_id, now + Duration::seconds(1))
            .await
            .unwrap()
            .heartbeat_at,
        now + Duration::seconds(1)
    );

    let resolved_at = now + Duration::seconds(2);
    let (resolved, resolution_event) = store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "approved".to_string(),
                selected_option_id: Some("allow".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "approved",
                    "outcome": "approved"
                }),
                resolved_at,
            },
        )
        .await
        .unwrap();
    assert_eq!(resolved.status, "approved");
    assert_eq!(resolved.resolution_version, 1);
    assert_eq!(resolved.resolution_event_seq, Some(resolution_event.seq));
    assert_eq!(resolution_event.event_type, "approval_resolved");
    assert!(matches!(
        store
            .resolve_approval_request(
                session.id,
                approval_id,
                NewApprovalResolution {
                    status: "denied".to_string(),
                    selected_option_id: Some("deny".to_string()),
                    resolution_json: json!({"status": "denied", "outcome": "denied"}),
                    resolved_at,
                },
            )
            .await,
        Err(ServerError::Conflict(_))
    ));

    let first_owner = Uuid::new_v4();
    let first = store
        .claim_approval_effect(approval_id, first_owner, resolved_at, Duration::seconds(30))
        .await
        .unwrap()
        .expect("resolved approval effect");
    assert_eq!(first.effect.effect_type, "memory_write");
    assert_eq!(first.effect.payload_json["proposalId"], "proposal-1");
    assert_eq!(
        first.effect.payload_json["resolution"]["outcome"],
        "approved"
    );
    assert!(
        store
            .claim_approval_effect(
                approval_id,
                Uuid::new_v4(),
                resolved_at + Duration::seconds(10),
                Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none()
    );

    let second = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            resolved_at + Duration::seconds(31),
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("stale effect lease is reclaimed");
    assert!(
        store
            .complete_approval_effect_with_event(
                &first,
                json!({"proposalId": "proposal-1", "status": "approved"}),
                None,
                resolved_at + Duration::seconds(31),
            )
            .await
            .is_err(),
        "the reclaimed lease fences the old owner"
    );
    let (applied, terminal_event) = store
        .complete_approval_effect_with_event(
            &second,
            json!({"proposalId": "proposal-1", "status": "approved"}),
            None,
            resolved_at + Duration::seconds(32),
        )
        .await
        .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(terminal_event.event_type, "write_proposal");
    assert!(
        store
            .claim_approval_effect(
                approval_id,
                Uuid::new_v4(),
                resolved_at + Duration::seconds(60),
                Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn in_memory_approval_effect_retry_commits_one_mutation_and_terminal_event() {
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
    let now = Utc::now();
    let approval_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
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
            effect_payload_json: json!({"proposalId": proposal_id}),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    let resolved_at = now + Duration::seconds(1);
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
                resolved_at,
            },
        )
        .await
        .unwrap();
    let first = store
        .claim_approval_effect(
            approval_id,
            Uuid::new_v4(),
            resolved_at,
            Duration::seconds(30),
        )
        .await
        .unwrap()
        .expect("first effect lease");
    let record = RecallChunkRecord {
        id: proposal_id,
        scope: "global".to_string(),
        text: "one durable reminder".to_string(),
        source: "fault-test".to_string(),
        importance: 1.0,
        created_at: resolved_at,
    };

    // Simulate a process dying after the idempotent mutation but before finalization.
    store.upsert_recall_chunk(record.clone()).await.unwrap();
    let retry_at = resolved_at + Duration::seconds(31);
    let retry = store
        .claim_approval_effect(approval_id, Uuid::new_v4(), retry_at, Duration::seconds(30))
        .await
        .unwrap()
        .expect("stale effect is reclaimed");
    store.upsert_recall_chunk(record).await.unwrap();

    let terminal_payload = json!({
        "kind": "memory",
        "proposalId": proposal_id,
        "status": "approved",
    });
    assert!(
        store
            .complete_approval_effect_with_event(&first, terminal_payload.clone(), None, retry_at,)
            .await
            .is_err(),
        "the stale owner must not append an event"
    );
    let (applied, event) = store
        .complete_approval_effect_with_event(&retry, terminal_payload.clone(), None, retry_at)
        .await
        .unwrap();
    assert_eq!(applied.status, "applied");
    assert_eq!(event.event_type, "write_proposal");
    assert_eq!(event.payload_json, terminal_payload);
    assert!(
        store
            .complete_approval_effect_with_event(
                &retry,
                json!({"status": "approved"}),
                None,
                retry_at,
            )
            .await
            .is_err(),
        "an applied effect cannot append a second event"
    );
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
}

#[tokio::test]
async fn in_memory_stale_approval_recovery_cancels_only_non_resumable_requests() {
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
    let now = Utc::now();
    let created_at = now - Duration::minutes(2);
    let non_resumable_id = Uuid::new_v4();
    let resumable_id = Uuid::new_v4();
    let expired_id = Uuid::new_v4();
    for (id, resumable) in [(non_resumable_id, false), (resumable_id, true)] {
        store
            .create_approval_request(NewApprovalRequest {
                id,
                session_id: session.id,
                turn_id: None,
                requester_id: Uuid::new_v4(),
                origin: "test".to_string(),
                action: "code.run".to_string(),
                scope_json: json!({}),
                options_json: json!([]),
                effect_type: "approval_continuation".to_string(),
                effect_payload_json: json!({}),
                resumable,
                created_at,
                expires_at: now + Duration::minutes(5),
            })
            .await
            .unwrap();
    }
    store
        .create_approval_request(NewApprovalRequest {
            id: expired_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "test".to_string(),
            action: "memory.write".to_string(),
            scope_json: json!({}),
            options_json: json!([]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({"origin": "test"}),
            resumable: true,
            created_at,
            expires_at: now - Duration::minutes(1),
        })
        .await
        .unwrap();

    let expiry_events = store.expire_pending_approvals(now).await.unwrap();
    assert_eq!(expiry_events.len(), 1);
    assert_eq!(expiry_events[0].event_type, "approval_resolved");
    assert_eq!(
        expiry_events[0].payload_json["approvalId"],
        json!(expired_id)
    );
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
            .expire_pending_approvals(now)
            .await
            .unwrap()
            .is_empty()
    );
    let expiry_worker = Uuid::new_v4();
    let expired_effect = store
        .claim_next_approval_effect(expiry_worker, now, Duration::seconds(30))
        .await
        .unwrap()
        .expect("expiry unblocks a worker-visible effect");
    assert_eq!(expired_effect.effect.approval_id, expired_id);
    store
        .complete_approval_effect(&expired_effect, now)
        .await
        .unwrap();

    let cancellation_events = store
        .cancel_stale_non_resumable_approvals(now - Duration::minutes(1), now)
        .await
        .unwrap();
    assert_eq!(cancellation_events.len(), 1);
    assert_eq!(
        cancellation_events[0].payload_json["approvalId"],
        json!(non_resumable_id)
    );
    assert_eq!(
        store
            .approval_request(session.id, non_resumable_id)
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    assert_eq!(
        store
            .approval_request(session.id, resumable_id)
            .await
            .unwrap()
            .status,
        "pending"
    );
    assert!(
        store
            .claim_approval_effect(non_resumable_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .claim_approval_effect(resumable_id, Uuid::new_v4(), now, Duration::seconds(30),)
            .await
            .unwrap()
            .is_none()
    );
}
