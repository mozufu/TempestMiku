use super::*;

#[tokio::test]
async fn memory_write_proposal_fails_closed_when_self_evolution_is_off() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_self_evolution_tier(tm_host::SelfEvolutionTier::Off);
    let (app, _) = test_app_with_state(state);
    let session = create(&app).await;
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/memory/proposals", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "memoryKind": "profile_fact",
                        "predicate": "prefers",
                        "object": "writes while disabled"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        response_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("evolution_disabled")
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());
    assert!(
        store
            .events_after(session.id, None)
            .await
            .unwrap()
            .iter()
            .all(|event| event.event_type != "write_proposal")
    );
    let audits = store.evolution_audits(session.id).await.unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].status, tm_host::EvolutionAuditStatus::Denied);
    assert_eq!(
        audits[0].error_code,
        Some(tm_host::EvolutionPolicyReason::DisabledTier)
    );
    assert!(audits[0].approval_id.is_none() && audits[0].effect_id.is_none());
}
#[tokio::test]
async fn memory_write_proposal_rejects_sensitive_data_before_emitting_events() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/memory/proposals", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "memoryKind": "recall_chunk",
                        "text": "Reminder: rotate sk-testsecret123456 tomorrow"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().all(|event| {
        event.event_type != "write_proposal"
            && (event.event_type != "approval" || event.payload_json["backend"] != json!("memory"))
    }));
}

#[tokio::test]
async fn memory_write_proposal_denial_does_not_persist() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
        "predicate": "likes",
        "object": "denied memory",
        "timeoutMs": 5000
    });
    let proposal = tokio::spawn(async move {
        post_app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{session_id}/memory/proposals"))
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    let approval_id = wait_for_event_payload(&store, session_id, "approval").await["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"decision":"deny"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::OK);
    let response = proposal.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("denied"));
    assert_eq!(store.profile_facts("brian").await.unwrap().len(), 0);
    assert!(
        store
            .active_memory_records("brian", "global", 5)
            .await
            .unwrap()
            .is_empty()
    );
    let events = store.events_after(session_id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "write_proposal" && event.payload_json["status"] == json!("denied")
    }));
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("memory")
            && event.payload_json["status"] == json!("denied")
    }));
}

#[tokio::test]
async fn memory_write_proposal_timeout_defaults_to_deny() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/memory/proposals", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "memoryKind": "recall_chunk",
                        "text": "this timed-out memory should not be saved",
                        "timeoutMs": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = response_json(res).await;
    assert_eq!(body["status"], json!("timed_out"));
    assert_eq!(
        store
            .recall_chunks("global", "timed-out memory", 5)
            .await
            .unwrap()
            .len(),
        0
    );
    let events = store.events_after(session.id, None).await.unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == "write_proposal" && event.payload_json["status"] == json!("timed_out")
    }));
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("memory")
            && event.payload_json["status"] == json!("timed_out")
            && event.payload_json["optionId"] == json!("reject")
    }));
}

#[tokio::test]
async fn approved_memory_writes_are_idempotent_by_dedupe_key() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let mut record_ids = Vec::new();
    for approval_index in 0..2 {
        let post_app = app.clone();
        let request = json!({
            "memoryKind": "recall_chunk",
            "text": "Stable durable memory chunk",
            "source": format!("test-source-{approval_index}"),
            "timeoutMs": 5000
        });
        let proposal = tokio::spawn(async move {
            post_app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/sessions/{session_id}/memory/proposals"))
                        .header("content-type", "application/json")
                        .body(Body::from(request.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        let approval =
            wait_for_nth_event_payload(&store, session_id, "approval", approval_index).await;
        let approval_id = approval["approvalId"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let approved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{session_id}/approvals/{approval_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"decision":"approve"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        let response = proposal.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["status"], json!("approved"));
        record_ids.push(body["record"]["id"].as_str().unwrap().to_string());
    }

    assert_eq!(record_ids[0], record_ids[1]);
    let chunks = store
        .recall_chunks("global", "Stable durable memory", 10)
        .await
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, "Stable durable memory chunk");
    let approved_proposals = store
        .events_after(session_id, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["status"] == json!("approved")
        })
        .count();
    assert_eq!(approved_proposals, 2);
}

#[tokio::test]
async fn approved_recall_chunks_remain_scope_isolated() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create_project_session(&app).await;
    let session_id = session.id;
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "recall_chunk",
        "text": "Project-only release checklist lives here",
        "source": "test-scope-isolation",
        "timeoutMs": 5000
    });
    let proposal =
        tokio::spawn(async move { post_memory_proposal(&post_app, session_id, request).await });
    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "approve").await;
    let response = proposal.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("approved"));
    assert!(
        body["record"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("memory://scopes/project:tempestmiku/chunks/")
    );

    let scoped = store
        .recall_chunks("project:tempestmiku", "release checklist", 10)
        .await
        .unwrap();
    assert_eq!(scoped.len(), 1);
    let global = store
        .recall_chunks("global", "release checklist", 10)
        .await
        .unwrap();
    assert!(global.is_empty());
}
