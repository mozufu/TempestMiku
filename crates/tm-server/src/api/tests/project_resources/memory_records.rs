use super::*;

#[tokio::test]
async fn memory_resource_gateway_reads_root_user_model_and_records() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let fact_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    store
        .add_profile_fact(ProfileFactRecord {
            id: fact_id,
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "memory resource tests".to_string(),
            confidence: 0.95,
            importance: 0.72,
            provenance: "test".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        })
        .await
        .unwrap();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: chunk_id,
            scope: "global".to_string(),
            text: "scoped memory resource recall".to_string(),
            source: "test".to_string(),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let fact_uri = format!("memory://profile/brian/facts/{fact_id}");
    let chunk_uri = format!("memory://scopes/global/chunks/{chunk_id}");
    for (uri, expected) in [
        ("memory://root".to_string(), "memory://user-model"),
        ("memory://user-model".to_string(), "memory resource tests"),
        (fact_uri, "Predicate: prefers"),
        (chunk_uri, "scoped memory resource recall"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri={}",
                        session.id, uri
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let json = response_json(response).await;
        assert_eq!(json["uri"], json!(uri));
        assert!(
            json["content"].as_str().unwrap().contains(expected),
            "content for {uri}: {json}"
        );
    }

    let other_fact_id = Uuid::new_v4();
    store
        .add_profile_fact(ProfileFactRecord {
            id: other_fact_id,
            subject: "someone-else".to_string(),
            predicate: "prefers".to_string(),
            object: "private data".to_string(),
            confidence: 0.95,
            importance: 0.72,
            provenance: "test".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        })
        .await
        .unwrap();
    let other_chunk_id = Uuid::new_v4();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: other_chunk_id,
            scope: "project:other".to_string(),
            text: "other project private recall".to_string(),
            source: "test".to_string(),
            importance: 0.65,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    for uri in [
        format!("memory://profile/someone-else/facts/{other_fact_id}"),
        format!("memory://scopes/project%3Aother/chunks/{other_chunk_id}"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri={uri}",
                        session.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn memory_resource_gateway_reads_dream_summaries_and_skill_proposals() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let dream_id = store
        .enqueue_dream(NewDreamQueueRecord {
            session_id: session.id,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            reason: DreamReason::ManualReflect,
            dedupe_key: format!("dream:summary-resource:{}", session.id),
            source_event_seq: None,
            available_at: Utc::now(),
        })
        .await
        .unwrap()
        .id;
    let evidence = vec![MemoryEvidenceRef {
        session_id: session.id,
        event_seq: Some(2),
        message_seq: Some(1),
        uri: Some("artifact://0".to_string()),
        label: "test-evidence".to_string(),
    }];
    let summary = store
        .upsert_memory_summary(NewMemorySummaryRecord {
            kind: MemorySummaryKind::Session,
            subject: "brian".to_string(),
            scope: "global".to_string(),
            title: "Dream summary resource".to_string(),
            body: "Summary body with provenance evidence.".to_string(),
            evidence: evidence.clone(),
            source_dream_id: dream_id,
            source_session_id: Some(session.id),
            dedupe_key: format!("summary:test:{}", session.id),
        })
        .await
        .unwrap();
    let skill = store
        .upsert_skill_proposal(NewSkillProposalRecord {
            name: "dream-test-workflow".to_string(),
            description: "Test skill proposal".to_string(),
            body: "# dream-test-workflow\n\n## Trigger\nTest\n\n## Procedure\n- Do it.\n"
                .to_string(),
            trigger: "Test recurring workflow".to_string(),
            use_criteria: "Only in tests".to_string(),
            evidence,
            self_critique: "Narrow enough for review.".to_string(),
            verification: SkillVerification {
                passed: true,
                checks: vec!["shape:pass".to_string()],
            },
            dedupe_key: format!("skill:test:{}", session.id),
            source_dream_id: dream_id,
            source_session_id: session.id,
        })
        .await
        .unwrap();

    for (uri, expected) in [
        (
            format!("memory://summaries/{}", summary.id),
            "Summary body with provenance evidence.",
        ),
        (
            format!("memory://skill-proposals/{}", skill.id),
            "Test skill proposal",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri={}",
                        session.id, uri
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let json = response_json(response).await;
        assert_eq!(json["uri"], json!(uri));
        assert!(
            json["content"].as_str().unwrap().contains(expected),
            "content for {uri}: {json}"
        );
    }

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=memory://summaries",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"] == json!(format!("memory://summaries/{}", summary.id)))
    );

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=memory://skill-proposals",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert!(
        listed.as_array().unwrap().iter().any(|entry| {
            entry["uri"] == json!(format!("memory://skill-proposals/{}", skill.id))
        })
    );

    let preview = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/preview?uri=memory://skill-proposals/{}",
                    session.id, skill.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = response_json(preview).await;
    assert_eq!(preview["content"], json!(""));
    assert!(
        preview["preview"]
            .as_str()
            .unwrap()
            .contains("Installable: false")
    );
}

#[tokio::test]
async fn memory_resource_gateway_lists_and_authorizes_evolution_episodes() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let turn = store
        .enqueue_turn(session.id, "evolution-resource", "inspect")
        .await
        .unwrap();
    let (episode, inserted) = store
        .upsert_evolution_episode(NewEvolutionEpisodeRecord {
            session_id: session.id,
            turn_id: turn.id,
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
        })
        .await
        .unwrap();
    assert!(inserted);
    store
        .replace_experience_traces(
            episode.id,
            vec![NewExperienceTraceRecord {
                episode_id: episode.id,
                ordinal: 0,
                kind: TraceKind::Terminal,
                capability: None,
                action_summary: "turn terminal".to_string(),
                observation_summary: "done".to_string(),
                error_signature: None,
                event_seq: 1,
                result_event_seq: None,
            }],
        )
        .await
        .unwrap();
    let uri = format!("memory://evolution/episodes/{}", episode.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={uri}",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["uri"], json!(uri));
    assert_eq!(json["kind"], json!("evolution_episode"));
    let content: Value = serde_json::from_str(json["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["episode"]["id"], json!(episode.id));
    assert_eq!(content["traces"][0]["observationSummary"], json!("done"));

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=memory://evolution/episodes",
                    session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert!(listed.as_array().unwrap().iter().any(|entry| {
        entry["uri"] == json!(uri) && entry["kind"] == json!("evolution_episode")
    }));

    let other_session = create(&app).await;
    store
        .set_session_memory_context(
            other_session.id,
            Some("other"),
            crate::MemoryPolicy::Project,
        )
        .await
        .unwrap();
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/resolve?uri={uri}",
                    other_session.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
}
