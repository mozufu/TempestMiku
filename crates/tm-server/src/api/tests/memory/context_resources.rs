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
