use super::*;

#[tokio::test]
async fn memory_context_injects_profile_facts_and_recall_chunks() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    store
        .add_profile_fact(ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "boring Rust".to_string(),
            confidence: 0.9,
            provenance: "test".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        })
        .await
        .unwrap();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "hello project open loop".to_string(),
            source: "test".to_string(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let session = create(&app).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let events = store.events_after(session.id, None).await.unwrap();
    let final_event = events
        .iter()
        .find(|event| event.event_type == "final")
        .unwrap();
    assert!(final_event.payload_json.to_string().contains("boring Rust"));
    assert!(
        final_event
            .payload_json
            .to_string()
            .contains("hello project open loop")
    );
}

#[tokio::test]
async fn memory_write_proposal_approval_persists_profile_fact_and_replays() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
        "subject": "brian",
        "predicate": "prefers",
        "object": "approval-backed memory writes",
        "confidence": 0.91,
        "provenanceLabel": "user-confirmed",
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

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["kind"], json!("memory"));
    assert_eq!(pending["memoryKind"], json!("profile_fact"));
    assert_eq!(pending["status"], json!("pending"));
    assert_eq!(pending["provenanceLabel"], json!("user-confirmed"));
    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    assert_eq!(
        approval["scope"]["proposal"]["proposalId"],
        pending["proposalId"]
    );
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
    let record_uri = body["record"]["uri"].as_str().unwrap().to_string();
    assert!(
        record_uri.starts_with("memory://profile/brian/facts/"),
        "{record_uri}"
    );

    let facts = store.profile_facts("brian").await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "prefers");
    assert_eq!(facts[0].object, "approval-backed memory writes");
    assert_eq!(facts[0].provenance, "user-confirmed");

    let record = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{session_id}/resources/resolve?uri={record_uri}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(record.status(), StatusCode::OK);
    let record_json = response_json(record).await;
    assert_eq!(record_json["uri"], json!(record_uri));
    assert!(
        record_json["content"]
            .as_str()
            .unwrap()
            .contains("approval-backed memory writes")
    );

    let preview = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{session_id}/resources/preview?uri={record_uri}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let preview_json = response_json(preview).await;
    assert_eq!(preview_json["content"], json!(""));
    assert!(
        preview_json["preview"]
            .as_str()
            .unwrap()
            .contains("approval-backed memory writes")
    );

    let events = store.events_after(session_id, None).await.unwrap();
    let write_statuses = events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .map(|event| event.payload_json["status"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(write_statuses, vec!["pending", "approved"]);
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("memory")
            && event.payload_json["status"] == json!("approved")
    }));
    let pending_seq = events
        .iter()
        .find(|event| {
            event.event_type == "write_proposal" && event.payload_json["status"] == json!("pending")
        })
        .unwrap()
        .seq;
    let replay = store
        .events_after(session_id, Some(pending_seq - 1))
        .await
        .unwrap();
    assert_eq!(replay[0].event_type, "write_proposal");
    assert_eq!(replay[0].payload_json["status"], json!("pending"));
}

#[tokio::test]
async fn memory_write_proposal_denial_does_not_persist() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
        "subject": "brian",
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
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
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
                        "scope": "global",
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
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let mut record_ids = Vec::new();
    for approval_index in 0..2 {
        let post_app = app.clone();
        let request = json!({
            "memoryKind": "recall_chunk",
            "scope": "global",
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
async fn personal_assistant_state_capture_proposes_memory_through_approval_flow() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;

    post_user_message(
        &app,
        session_id,
        "Remember that I prefer approval-backed state capture summaries.",
    )
    .await;

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["kind"], json!("memory"));
    assert_eq!(pending["memoryKind"], json!("profile_fact"));
    assert_eq!(pending["status"], json!("pending"));
    assert_eq!(
        pending["provenanceLabel"],
        json!("personal-assistant-state-capture")
    );
    assert_eq!(pending["predicate"], json!("prefers"));
    assert_eq!(
        pending["object"],
        json!("approval-backed state capture summaries")
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());

    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    assert_eq!(
        approval["scope"]["proposal"]["proposalId"],
        pending["proposalId"]
    );
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "approve").await;

    let approved =
        wait_for_write_proposal_status(&store, session_id, MemoryWriteStatus::Approved).await;
    assert_eq!(approved["record"]["kind"], json!("profile_fact"));
    assert!(
        approved["record"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("memory://profile/brian/facts/")
    );
    let facts = store.profile_facts("brian").await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "prefers");
    assert_eq!(facts[0].object, "approval-backed state capture summaries");
    assert_eq!(facts[0].provenance, "personal-assistant-state-capture");
}

#[tokio::test]
async fn personal_assistant_state_capture_does_not_propose_sensitive_or_transient_memory() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(&app, session.id, "Please remember my password is hunter2.").await;
    post_user_message(
        &app,
        session.id,
        "Just venting: that meeting was annoying and I am grumpy.",
    )
    .await;

    let events = store.events_after(session.id, None).await.unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "write_proposal"),
        "sensitive/transient personal-assistant prompts should not emit memory proposals"
    );
    assert!(
        events.iter().all(|event| event.event_type != "approval"
            || event.payload_json["backend"] != json!("memory")),
        "skipped capture should not request memory approval"
    );
    assert!(store.profile_facts("brian").await.unwrap().is_empty());
    assert!(
        store
            .recall_chunks("global", "password", 5)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn gated_postgres_memory_approval_flow_persists_and_replays() {
    let Some((app, store)) = postgres_test_app().await else {
        return;
    };

    let run_id = Uuid::new_v4().simple().to_string();
    let subject = "brian".to_string();

    let approved_session = create(&app).await;
    let approved_session_id = approved_session.id;
    let profile_object = format!("approval-backed postgres memory {run_id}");
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
        "subject": subject.clone(),
        "predicate": "prefers",
        "object": profile_object.clone(),
        "confidence": 0.93,
        "provenanceLabel": "postgres-approved",
        "timeoutMs": 5000
    });
    let proposal =
        tokio::spawn(
            async move { post_memory_proposal(&post_app, approved_session_id, request).await },
        );

    let pending = wait_for_event_payload(&store, approved_session_id, "write_proposal").await;
    assert_eq!(pending["kind"], json!("memory"));
    assert_eq!(pending["memoryKind"], json!("profile_fact"));
    assert_eq!(pending["status"], json!("pending"));
    let approval = wait_for_event_payload(&store, approved_session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    assert_eq!(
        approval["scope"]["proposal"]["proposalId"],
        pending["proposalId"]
    );
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, approved_session_id, approval_id, "approve").await;

    let response = proposal.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("approved"));
    let record_uri = body["record"]["uri"].as_str().unwrap().to_string();
    assert!(
        record_uri.starts_with("memory://profile/brian/facts/"),
        "{record_uri}"
    );
    let profile_record_id = body["record"]["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let fact = store
        .profile_fact(&subject, profile_record_id)
        .await
        .unwrap();
    assert_eq!(fact.predicate, "prefers");
    assert_eq!(fact.object, profile_object);
    assert_eq!(fact.provenance, "postgres-approved");
    let raw_fact_count: i64 = store
            .client()
            .query_one(
                "select count(*) from profile_facts where id = $1 and subject = $2 and object = $3 and valid_to is null",
                &[&profile_record_id, &subject, &profile_object],
            )
            .await
            .unwrap()
            .get(0);
    assert_eq!(raw_fact_count, 1);

    let root_json =
        get_session_resource_json(&app, approved_session_id, "resolve", "memory://root").await;
    assert_eq!(root_json["uri"], json!("memory://root"));
    assert!(
        root_json["content"]
            .as_str()
            .unwrap()
            .contains(&profile_object)
    );
    let user_model_json =
        get_session_resource_json(&app, approved_session_id, "resolve", "memory://user-model")
            .await;
    assert_eq!(user_model_json["uri"], json!("memory://user-model"));
    assert!(
        user_model_json["content"]
            .as_str()
            .unwrap()
            .contains(&profile_object)
    );
    let record_json =
        get_session_resource_json(&app, approved_session_id, "resolve", &record_uri).await;
    assert_eq!(record_json["uri"], json!(record_uri));
    assert!(
        record_json["content"]
            .as_str()
            .unwrap()
            .contains(&profile_object)
    );
    for uri in ["memory://root", "memory://user-model", record_uri.as_str()] {
        let preview_json =
            get_session_resource_json(&app, approved_session_id, "preview", uri).await;
        assert_eq!(preview_json["uri"], json!(uri));
        assert_eq!(preview_json["content"], json!(""));
        assert!(
            preview_json["preview"].as_str().unwrap().len() <= SESSION_RESOURCE_PREVIEW_BYTES + 64
        );
    }

    let events = store.events_after(approved_session_id, None).await.unwrap();
    let write_statuses = events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .map(|event| event.payload_json["status"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(write_statuses, vec!["pending", "approved"]);
    assert!(events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("memory")
            && event.payload_json["status"] == json!("approved")
    }));
    let pending_seq = events
        .iter()
        .find(|event| {
            event.event_type == "write_proposal" && event.payload_json["status"] == json!("pending")
        })
        .unwrap()
        .seq;
    let replay = store
        .events_after(approved_session_id, Some(pending_seq - 1))
        .await
        .unwrap();
    assert_eq!(replay[0].event_type, "write_proposal");
    assert_eq!(replay[0].payload_json["status"], json!("pending"));
    let replay_write_statuses = replay
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .map(|event| event.payload_json["status"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(replay_write_statuses, vec!["pending", "approved"]);

    let chunk_session = create(&app).await;
    let chunk_session_id = chunk_session.id;
    let scope = format!("postgres-p2-{run_id}");
    let chunk_text = format!("Stable durable postgres memory chunk {run_id}");
    let mut record_ids = Vec::new();
    let mut record_uris = Vec::new();
    for approval_index in 0..2 {
        let post_app = app.clone();
        let request = json!({
            "memoryKind": "recall_chunk",
            "subject": subject.clone(),
            "scope": scope.clone(),
            "text": chunk_text.clone(),
            "source": format!("postgres-test-source-{approval_index}"),
            "provenanceLabel": "postgres-recall",
            "timeoutMs": 5000
        });
        let proposal = tokio::spawn(async move {
            post_memory_proposal(&post_app, chunk_session_id, request).await
        });
        let approval =
            wait_for_nth_event_payload(&store, chunk_session_id, "approval", approval_index).await;
        let approval_id = approval["approvalId"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        resolve_test_approval(&app, chunk_session_id, approval_id, "approve").await;
        let response = proposal.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["status"], json!("approved"));
        record_ids.push(body["record"]["id"].as_str().unwrap().to_string());
        record_uris.push(body["record"]["uri"].as_str().unwrap().to_string());
    }
    assert_eq!(record_ids[0], record_ids[1]);
    assert_eq!(record_uris[0], record_uris[1]);
    let chunks = store.recall_chunks(&scope, &run_id, 10).await.unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, chunk_text);
    let chunk_record_id = record_ids[0].parse::<Uuid>().unwrap();
    let raw_chunk_count: i64 = store
        .client()
        .query_one(
            "select count(*) from recall_chunks where id = $1 and scope = $2 and text = $3",
            &[&chunk_record_id, &scope, &chunk_text],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(raw_chunk_count, 1);
    let chunk_record_json =
        get_session_resource_json(&app, chunk_session_id, "resolve", record_uris[0].as_str()).await;
    assert_eq!(chunk_record_json["uri"], json!(record_uris[0]));
    assert!(
        chunk_record_json["content"]
            .as_str()
            .unwrap()
            .contains(&chunk_text)
    );
    let chunk_preview_json =
        get_session_resource_json(&app, chunk_session_id, "preview", record_uris[0].as_str()).await;
    assert_eq!(chunk_preview_json["content"], json!(""));
    assert!(
        chunk_preview_json["preview"].as_str().unwrap().len()
            <= SESSION_RESOURCE_PREVIEW_BYTES + 64
    );
    let approved_chunk_proposals = store
        .events_after(chunk_session_id, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.event_type == "write_proposal"
                && event.payload_json["status"] == json!("approved")
        })
        .count();
    assert_eq!(approved_chunk_proposals, 2);

    let denied_session = create(&app).await;
    let denied_session_id = denied_session.id;
    let denied_subject = format!("brian-denied-pg-{run_id}");
    let denied_object = format!("denied postgres memory {run_id}");
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
        "subject": denied_subject.clone(),
        "predicate": "likes",
        "object": denied_object.clone(),
        "timeoutMs": 5000
    });
    let proposal =
        tokio::spawn(
            async move { post_memory_proposal(&post_app, denied_session_id, request).await },
        );
    let approval = wait_for_event_payload(&store, denied_session_id, "approval").await;
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, denied_session_id, approval_id, "deny").await;
    let response = proposal.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("denied"));
    assert!(body["record"].is_null());
    assert_eq!(store.profile_facts(&denied_subject).await.unwrap().len(), 0);
    let raw_denied_count: i64 = store
        .client()
        .query_one(
            "select count(*) from profile_facts where subject = $1 and object = $2",
            &[&denied_subject, &denied_object],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(raw_denied_count, 0);
    let denied_events = store.events_after(denied_session_id, None).await.unwrap();
    let denied_statuses = denied_events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .map(|event| event.payload_json["status"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(denied_statuses, vec!["pending", "denied"]);
    assert!(denied_events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("memory")
            && event.payload_json["status"] == json!("denied")
    }));

    let timeout_session = create(&app).await;
    let timeout_session_id = timeout_session.id;
    let timeout_scope = format!("postgres-timeout-{run_id}");
    let timeout_text = format!("timed-out postgres memory should not persist {run_id}");
    let response = post_memory_proposal(
        &app,
        timeout_session_id,
        json!({
            "memoryKind": "recall_chunk",
            "subject": subject.clone(),
            "scope": timeout_scope.clone(),
            "text": timeout_text.clone(),
            "timeoutMs": 1
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], json!("timed_out"));
    assert!(body["record"].is_null());
    assert_eq!(
        store
            .recall_chunks(&timeout_scope, "timed-out postgres memory", 10)
            .await
            .unwrap()
            .len(),
        0
    );
    let raw_timeout_count: i64 = store
        .client()
        .query_one(
            "select count(*) from recall_chunks where scope = $1 and text = $2",
            &[&timeout_scope, &timeout_text],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(raw_timeout_count, 0);
    let timeout_events = store.events_after(timeout_session_id, None).await.unwrap();
    let timeout_statuses = timeout_events
        .iter()
        .filter(|event| event.event_type == "write_proposal")
        .map(|event| event.payload_json["status"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(timeout_statuses, vec!["pending", "timed_out"]);
    assert!(timeout_events.iter().any(|event| {
        event.event_type == "approval_resolved"
            && event.payload_json["backend"] == json!("memory")
            && event.payload_json["status"] == json!("timed_out")
            && event.payload_json["optionId"] == json!("reject")
    }));
}
