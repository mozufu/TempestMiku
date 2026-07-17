use super::*;

#[tokio::test]
async fn memory_write_proposal_approval_persists_profile_fact_and_replays() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let post_app = app.clone();
    let request = json!({
        "memoryKind": "profile_fact",
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
    assert!(pending.get("provenanceLabel").is_none());
    assert!(
        pending["uri"]
            .as_str()
            .unwrap()
            .starts_with("memory://evolution-proposals/")
    );
    assert!(
        pending["contentDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let candidate_uri = pending["uri"].as_str().unwrap();
    let candidate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{session_id}/resources/resolve?uri={candidate_uri}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(candidate_response.status(), StatusCode::OK);
    let candidate_resource = response_json(candidate_response).await;
    assert!(
        candidate_resource["content"]
            .as_str()
            .unwrap()
            .contains("user-confirmed")
    );
    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    assert_eq!(
        approval["scope"]["proposal"]["proposalId"],
        pending["proposalId"]
    );
    let audit_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{session_id}/resources/resolve?uri=memory://evolution-audits"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit_response.status(), StatusCode::OK);
    let audit_resource = response_json(audit_response).await;
    let audit_content = audit_resource["content"].as_str().unwrap();
    assert!(audit_content.contains("awaiting_approval"));
    assert!(!audit_content.contains("user-confirmed"));
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
    let typed = store
        .memory_record(
            "brian",
            "global",
            tm_memory::MemoryRecordKind::Semantic,
            facts[0].id,
        )
        .await
        .unwrap();
    let tm_memory::MemoryRecordResource::Semantic(typed) = typed.resource else {
        panic!("approved profile fact must create a typed semantic record");
    };
    assert_eq!(typed.status, tm_memory::MemoryRecordStatus::Active);
    assert_eq!(typed.evidence[0].label, "user-confirmed");
    assert_eq!(
        typed.evidence[0].source,
        tm_memory::MemoryEvidenceSource::Resource {
            uri: pending["uri"].as_str().unwrap().to_string()
        }
    );

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
async fn approved_profile_fact_contradiction_supersedes_without_erasing_history() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;
    let mut record_uris = Vec::new();

    for (approval_index, object) in [
        "small, reviewable patches",
        "larger design-first changes when risk is high",
    ]
    .into_iter()
    .enumerate()
    {
        let post_app = app.clone();
        let request = json!({
            "memoryKind": "profile_fact",
            "predicate": "prefers",
            "object": object,
            "confidence": 0.9,
            "provenanceLabel": format!("contradiction-{approval_index}"),
            "timeoutMs": 5000
        });
        let proposal =
            tokio::spawn(async move { post_memory_proposal(&post_app, session_id, request).await });
        let approval =
            wait_for_nth_event_payload(&store, session_id, "approval", approval_index).await;
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
        record_uris.push(body["record"]["uri"].as_str().unwrap().to_string());
    }

    let facts = store.profile_facts("brian").await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "prefers");
    assert_eq!(
        facts[0].object,
        "larger design-first changes when risk is high"
    );
    assert_eq!(facts[0].valid_to, None);

    let old = get_session_resource_json(&app, session_id, "resolve", &record_uris[0]).await;
    let old_content = old["content"].as_str().unwrap();
    assert!(old_content.contains("small, reviewable patches"));
    assert!(old_content.contains("Valid to: "));
    assert!(!old_content.contains("Valid to: active"));

    let current = get_session_resource_json(&app, session_id, "resolve", &record_uris[1]).await;
    let current_content = current["content"].as_str().unwrap();
    assert!(current_content.contains("larger design-first changes when risk is high"));
    assert!(current_content.contains("Valid to: active"));

    let legacy_context = StoreMemoryProvider::new(Arc::clone(&store))
        .context_for_turn("brian", "global", "prefers")
        .await
        .unwrap()
        .render_prompt_block();
    assert!(!legacy_context.contains("small, reviewable patches"));
    assert!(legacy_context.contains("larger design-first changes when risk is high"));
}
