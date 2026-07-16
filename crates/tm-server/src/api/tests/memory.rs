use super::*;

struct HybridTraceProvider {
    candidate: tm_memory::HybridMemoryCandidate,
}

#[async_trait]
impl MemoryProvider for HybridTraceProvider {
    async fn context_for_turn(
        &self,
        subject: &str,
        scope: &str,
        _query: &str,
    ) -> Result<crate::MemoryContext> {
        Ok(crate::MemoryContext::from_hybrid_candidates_with_summaries(
            subject,
            scope,
            Vec::new(),
            vec![self.candidate.clone()],
            1_600,
            Some("emb-v1-api-fixture".to_string()),
        ))
    }
}

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
async fn memory_context_injects_profile_facts_and_recall_chunks() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    store
        .add_profile_fact(ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "boring Rust".to_string(),
            confidence: 0.9,
            importance: 0.72,
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
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let session = create(&app).await;
    post_user_message(&app, session.id, "hello").await;
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
async fn hybrid_turn_persists_exact_bounded_recall_and_typed_record_resources() {
    let store = Arc::new(InMemoryStore::default());
    let now = Utc::now();
    let record_id = Uuid::new_v4();
    let record = tm_memory::StoredMemoryRecord::new(tm_memory::MemoryRecordResource::Episodic(
        tm_memory::EpisodicMemoryRecord {
            schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
            id: record_id,
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            text: "The bounded hybrid trace survives event replay.".to_string(),
            evidence: vec![tm_memory::MemoryRecordEvidence::resource(
                "memory://evolution-proposals/api-fixture",
                "approved extraction",
            )],
            confidence: 0.9,
            importance: 0.8,
            observed_at: now,
            effective_from: now,
            effective_to: None,
            status: tm_memory::MemoryRecordStatus::Active,
            links: Default::default(),
            created_at: now,
        },
    ))
    .unwrap();
    store.upsert_memory_record(record.clone()).await.unwrap();
    let memory = Arc::new(HybridTraceProvider {
        candidate: tm_memory::HybridMemoryCandidate {
            record,
            lexical_rank: Some(2),
            lexical_score: Some(0.6),
            dense_rank: Some(1),
            dense_score: Some(0.9),
            embedding_version: Some("emb-v1-api-fixture".to_string()),
            rrf_score: 0.0325,
        },
    });
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        Arc::new(EchoChatRunner),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    let app = app(state);
    let session = create(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(message_body("show the replayable memory"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;
    let completed = wait_for_turn(&app, session.id, turn_id).await;
    assert_eq!(completed["status"], json!("completed"));

    let trace_event = store
        .event_for_turn(session.id, turn_id, "memory_recall")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        trace_event.payload_json["resourceUri"],
        json!(format!("memory://recalls/{turn_id}"))
    );
    assert_eq!(
        trace_event.payload_json["context"]["retrieval"]["mode"],
        json!("hybrid")
    );
    assert_eq!(
        trace_event.payload_json["context"]["retrieval"]["candidates"][0]["included"],
        json!(true)
    );

    let trace = get_session_resource_json(
        &app,
        session.id,
        "resolve",
        &format!("memory://recalls/{turn_id}"),
    )
    .await;
    assert_eq!(trace["mime"], json!("application/json"));
    let trace_content: Value = serde_json::from_str(trace["content"].as_str().unwrap()).unwrap();
    assert_eq!(trace_content, trace_event.payload_json);

    let record_uri = format!("memory://records/episodic/{record_id}");
    let record = get_session_resource_json(&app, session.id, "resolve", &record_uri).await;
    assert_eq!(record["mime"], json!("application/json"));
    let record_content: Value = serde_json::from_str(record["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        record_content["resource"]["record"]["evidence"][0]["label"],
        json!("approved extraction")
    );
    let preview = get_session_resource_json(&app, session.id, "preview", &record_uri).await;
    assert_eq!(preview["content"], json!(""));
    assert!(preview["preview"].as_str().unwrap().len() <= SESSION_RESOURCE_PREVIEW_BYTES + 64);

    let recalls = get_session_resource_json(&app, session.id, "list", "memory://recalls").await;
    assert!(
        recalls
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"] == json!(format!("memory://recalls/{turn_id}")))
    );

    let project_record_id = Uuid::new_v4();
    store
        .upsert_memory_record(
            tm_memory::StoredMemoryRecord::new(tm_memory::MemoryRecordResource::Episodic(
                tm_memory::EpisodicMemoryRecord {
                    schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
                    id: project_record_id,
                    owner_subject: "brian".to_string(),
                    memory_scope: "project:tempestmiku".to_string(),
                    text: "Project-scoped memory must not leak into the global scope.".to_string(),
                    evidence: vec![tm_memory::MemoryRecordEvidence::resource(
                        "memory://evolution-proposals/project-fixture",
                        "approved extraction",
                    )],
                    confidence: 0.9,
                    importance: 0.8,
                    observed_at: now,
                    effective_from: now,
                    effective_to: None,
                    status: tm_memory::MemoryRecordStatus::Active,
                    links: Default::default(),
                    created_at: now,
                },
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    let project_record_uri = format!("memory://records/episodic/{project_record_id}");
    let cross_scope = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={project_record_uri}",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_scope.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recall_trace_listing_filters_authority_before_applying_the_limit() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let authorized_turn = store
        .enqueue_turn(session.id, "authorized-recall", "authorized recall")
        .await
        .unwrap();
    store
        .append_event_for_turn(
            session.id,
            "memory_recall",
            json!({
                "context": {
                    "subject": "brian",
                    "scope": "global"
                }
            }),
            Some(authorized_turn.id),
        )
        .await
        .unwrap();
    for index in 0..crate::memory::DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT {
        let unauthorized_turn = store
            .enqueue_turn(
                session.id,
                &format!("unauthorized-recall-{index}"),
                "unauthorized recall",
            )
            .await
            .unwrap();
        store
            .append_event_for_turn(
                session.id,
                "memory_recall",
                json!({
                    "context": {
                        "subject": "brian",
                        "scope": "project:other"
                    }
                }),
                Some(unauthorized_turn.id),
            )
            .await
            .unwrap();
    }

    let recalls = get_session_resource_json(&app, session.id, "list", "memory://recalls").await;
    let recalls = recalls.as_array().unwrap();
    assert_eq!(recalls.len(), 1);
    assert_eq!(
        recalls[0]["uri"],
        json!(format!("memory://recalls/{}", authorized_turn.id))
    );
}

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

#[tokio::test]
async fn personal_assistant_state_capture_proposes_memory_through_approval_flow() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
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
        pending["preview"],
        json!("brian prefers approval-backed state capture summaries")
    );
    assert!(pending.get("predicate").is_none());
    assert!(pending.get("object").is_none());
    let proposal_id = pending["proposalId"].as_str().unwrap().parse().unwrap();
    let candidate = store.evolution_memory_proposal(proposal_id).await.unwrap();
    assert_eq!(candidate.predicate.as_deref(), Some("prefers"));
    assert_eq!(
        candidate.object.as_deref(),
        Some("approval-backed state capture summaries")
    );
    assert_eq!(
        candidate.provenance_label,
        "personal-assistant-state-capture"
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
    assert_eq!(facts[0].importance, 0.72);
}

#[tokio::test]
async fn personal_assistant_reminder_capture_persists_approved_recall_chunk() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;

    post_user_message(
        &app,
        session_id,
        "Remind me to review the P2 acceptance checklist by Friday.",
    )
    .await;

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["kind"], json!("memory"));
    assert_eq!(pending["memoryKind"], json!("recall_chunk"));
    assert_eq!(pending["status"], json!("pending"));
    assert_eq!(
        pending["preview"],
        json!("Reminder: review the P2 acceptance checklist by Friday")
    );
    assert!(pending.get("text").is_none());
    assert!(pending.get("provenance").is_none());
    let proposal_id = pending["proposalId"].as_str().unwrap().parse().unwrap();
    let candidate = store.evolution_memory_proposal(proposal_id).await.unwrap();
    assert_eq!(
        candidate.provenance["capturedCategory"],
        "personal_reminder"
    );
    assert_eq!(candidate.importance_score, 0.64);
    assert!(
        store
            .recall_chunks("global", "P2 acceptance checklist", 5)
            .await
            .unwrap()
            .is_empty()
    );

    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    assert_eq!(approval["backend"], json!("memory"));
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "approve").await;

    let approved =
        wait_for_write_proposal_status(&store, session_id, MemoryWriteStatus::Approved).await;
    assert_eq!(approved["record"]["kind"], json!("recall_chunk"));
    let record_uri = approved["record"]["uri"].as_str().unwrap();
    assert!(record_uri.starts_with("memory://scopes/global/chunks/"));
    let chunks = store
        .recall_chunks("global", "P2 acceptance checklist", 5)
        .await
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0].text,
        "Reminder: review the P2 acceptance checklist by Friday"
    );
    assert_eq!(chunks[0].importance, 0.64);

    let root_json = get_session_resource_json(&app, session_id, "resolve", "memory://root").await;
    assert!(
        root_json["content"]
            .as_str()
            .unwrap()
            .contains("P2 acceptance checklist")
    );
}

#[tokio::test]
async fn denied_personal_assistant_reminder_capture_does_not_persist() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let session_id = session.id;

    post_user_message(&app, session_id, "Reminder: send the P2 notes by Friday.").await;

    let pending = wait_for_event_payload(&store, session_id, "write_proposal").await;
    assert_eq!(pending["memoryKind"], json!("recall_chunk"));
    assert_eq!(pending["status"], json!("pending"));
    let approval = wait_for_event_payload(&store, session_id, "approval").await;
    let approval_id = approval["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    resolve_test_approval(&app, session_id, approval_id, "deny").await;

    let denied =
        wait_for_write_proposal_status(&store, session_id, MemoryWriteStatus::Denied).await;
    assert_eq!(denied["record"], json!(null));
    assert!(
        store
            .recall_chunks("global", "P2 notes", 5)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn personal_assistant_state_capture_does_not_propose_sensitive_or_transient_memory() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;

    post_user_message(&app, session.id, "Please remember my password is hunter2.").await;
    post_user_message(
        &app,
        session.id,
        "Reminder: rotate sk-testsecret123456 tomorrow.",
    )
    .await;
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
