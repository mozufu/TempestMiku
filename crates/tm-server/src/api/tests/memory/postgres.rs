use super::*;

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

    let unrelated_record_id = Uuid::new_v4();
    let unrelated_now = Utc::now();
    store
        .upsert_memory_record(
            tm_memory::StoredMemoryRecord::new(tm_memory::MemoryRecordResource::Semantic(
                tm_memory::SemanticMemoryRecord {
                    schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
                    id: unrelated_record_id,
                    owner_subject: subject.clone(),
                    memory_scope: "global".to_string(),
                    semantic_subject: "miku".to_string(),
                    predicate: "prefers".to_string(),
                    object: "unrelated semantic subject".to_string(),
                    evidence: vec![tm_memory::MemoryRecordEvidence::resource(
                        "memory://fixtures/unrelated-semantic-subject",
                        "postgres semantic subject isolation",
                    )],
                    confidence: 0.9,
                    importance: 0.7,
                    observed_at: unrelated_now,
                    effective_from: unrelated_now,
                    effective_to: None,
                    status: tm_memory::MemoryRecordStatus::Active,
                    links: Default::default(),
                    created_at: unrelated_now,
                },
            ))
            .unwrap(),
        )
        .await
        .unwrap();

    let replacement_session = create(&app).await;
    let replacement_session_id = replacement_session.id;
    let replacement_object = format!("replacement postgres memory {run_id}");
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
        "predicate": "prefers",
        "object": replacement_object,
        "confidence": 0.94,
        "provenanceLabel": "postgres-replacement",
        "timeoutMs": 5000
    });
    let replacement = tokio::spawn(async move {
        post_memory_proposal(&post_app, replacement_session_id, request).await
    });
    let replacement_approval =
        wait_for_event_payload(&store, replacement_session_id, "approval").await;
    let replacement_approval_id = replacement_approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(
        &app,
        replacement_session_id,
        replacement_approval_id,
        "approve",
    )
    .await;
    let replacement_response = replacement.await.unwrap();
    assert_eq!(replacement_response.status(), StatusCode::OK);
    let replacement_body = response_json(replacement_response).await;
    let replacement_record_id = replacement_body["record"]["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let previous_typed = store
        .memory_record(
            &subject,
            "global",
            tm_memory::MemoryRecordKind::Semantic,
            profile_record_id,
        )
        .await
        .unwrap();
    assert_eq!(
        previous_typed.resource.status(),
        tm_memory::MemoryRecordStatus::Superseded
    );
    assert_eq!(
        previous_typed.resource.links().superseded_by_record_id,
        Some(replacement_record_id)
    );
    let replacement_typed = store
        .memory_record(
            &subject,
            "global",
            tm_memory::MemoryRecordKind::Semantic,
            replacement_record_id,
        )
        .await
        .unwrap();
    assert_eq!(
        replacement_typed.resource.status(),
        tm_memory::MemoryRecordStatus::Active
    );
    assert_eq!(
        replacement_typed.resource.links().supersedes_record_id,
        Some(profile_record_id)
    );
    assert!(
        store
            .profile_fact(&subject, profile_record_id)
            .await
            .unwrap()
            .valid_to
            .is_some()
    );
    let unrelated = store
        .memory_record(
            &subject,
            "global",
            tm_memory::MemoryRecordKind::Semantic,
            unrelated_record_id,
        )
        .await
        .unwrap();
    assert_eq!(
        unrelated.resource.status(),
        tm_memory::MemoryRecordStatus::Active
    );
    assert_eq!(unrelated.resource.links().superseded_by_record_id, None);

    let chunk_session = create(&app).await;
    let chunk_session_id = chunk_session.id;
    let scope = "global".to_string();
    let chunk_text = format!("Stable durable postgres memory chunk {run_id}");
    let mut record_ids = Vec::new();
    let mut record_uris = Vec::new();
    for approval_index in 0..2 {
        let post_app = app.clone();
        let request = json!({
            "memoryKind": "recall_chunk",
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
    let denied_subject = subject.clone();
    let denied_object = format!("denied postgres memory {run_id}");
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
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
    assert!(
        store
            .profile_facts(&denied_subject)
            .await
            .unwrap()
            .iter()
            .all(|fact| fact.object != denied_object),
        "denied object must not be persisted alongside earlier approved facts"
    );
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
    let timeout_scope = "global".to_string();
    let timeout_text = format!("timed-out postgres memory should not persist {run_id}");
    let response = post_memory_proposal(
        &app,
        timeout_session_id,
        json!({
            "memoryKind": "recall_chunk",
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
