use super::*;

#[tokio::test]
async fn session_creation_message_append_event_append_and_replay_work() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    assert_eq!(
        session.active_skills,
        vec!["miku-voice", "personal-assistant-state-capture"]
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let reused = response_json(res).await;
    assert_eq!(reused["id"], session.id.to_string());
    assert_eq!(reused["mode"], json!("personal_assistant"));

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
    let all = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["mode", "text", "final"]
    );
    assert_eq!(
        all[0].payload_json["voice_cap"],
        serde_json::json!("medium")
    );
    assert_eq!(
        all[0].payload_json["activeSkills"],
        json!(["miku-voice", "personal-assistant-state-capture"])
    );
    let replay = store.events_after(session.id, Some(1)).await.unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["text", "final"]
    );
}

#[tokio::test]
async fn sessions_history_lists_recent_sessions_and_hydrates_transcript() {
    let (app, store) = test_app(PersonaConfig::default(), AuthConfig::NoAuth);
    let first = create(&app).await;
    post_user_message(&app, first.id, "first session asks for status").await;
    tokio::time::sleep(Duration::from_millis(2)).await;
    let second = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    post_user_message(&app, second.id, "second session asks for code").await;
    store
        .upsert_project_item(NewProjectItem {
            project_id: "tempestmiku".to_string(),
            kind: ProjectItemKind::Summary,
            text: "Condensed code-session summary".to_string(),
            target_uri: format!("project://tempestmiku/summary/{}", second.id),
            source_session_id: second.id,
            source_event_seq: None,
            source_uri: None,
            dedupe_key: format!("test-summary:{}", second.id),
            provenance_json: json!({"source": "test"}),
        })
        .await
        .unwrap();

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/sessions?limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], second.id.to_string());
    assert_eq!(sessions[0]["title"], "Condensed code-session summary");
    assert_eq!(sessions[0]["messageCount"], 2);
    assert!(sessions[0]["lastEventId"].as_i64().unwrap() >= 3);
    assert_eq!(sessions[1]["id"], first.id.to_string());
    assert_eq!(
        sessions[1]["title"],
        "Miku heard: first session asks for status"
    );

    store
        .append_event(
            first.id,
            "write_proposal",
            json!({
                "kind": "memory",
                "proposalId": "proposal-history",
                "status": "pending",
                "text": "Remember history restore."
            }),
        )
        .await
        .unwrap();
    store
        .append_event(
            first.id,
            "approval",
            json!({
                "approvalId": "approval-history",
                "backend": "memory",
                "action": "memory.write profile_fact",
                "scope": {}
            }),
        )
        .await
        .unwrap();

    let transcript = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/messages", first.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(transcript.status(), StatusCode::OK);
    let transcript = response_json(transcript).await;
    assert_eq!(transcript["id"], first.id.to_string());
    assert_eq!(transcript["messages"][0]["role"], "user");
    assert_eq!(
        transcript["messages"][0]["content"],
        "first session asks for status"
    );
    assert_eq!(transcript["messages"][1]["role"], "assistant");
    assert!(
        transcript["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("Miku heard: first session asks for status")
    );
    assert!(transcript["lastEventId"].as_i64().unwrap() >= 5);
    let pending = transcript["pendingEvents"].as_array().unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["write_proposal", "approval"]
    );

    store
        .append_event(
            first.id,
            "approval_resolved",
            json!({
                "approvalId": "approval-history",
                "status": "approved"
            }),
        )
        .await
        .unwrap();
    store
        .append_event(
            first.id,
            "write_proposal",
            json!({
                "kind": "memory",
                "proposalId": "proposal-history",
                "status": "approved"
            }),
        )
        .await
        .unwrap();
    let resolved = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/messages", first.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let resolved = response_json(resolved).await;
    assert!(resolved["pendingEvents"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn missing_persona_path_boots_degraded_with_warning() {
    let (app, _) = test_app(
        PersonaConfig::from_path("/definitely/missing/tempestmiku/persona"),
        AuthConfig::NoAuth,
    );
    let session = create(&app).await;
    match session.persona_status {
        crate::PersonaStatus::Degraded { warning } => assert!(warning.contains("missing")),
        crate::PersonaStatus::Loaded { .. } => panic!("missing path must degrade"),
    }
}

#[tokio::test]
async fn chat_turn_prompt_uses_active_persona_bundle() {
    let temp = tempfile::tempdir().unwrap();
    write_persona_fixture(temp.path());
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let turns = Arc::clone(&chat.turns);
    let state = AppState::new(
        store,
        memory,
        chat,
        PersonaConfig::from_path(temp.path()),
        AuthConfig::NoAuth,
    );
    let app = app(state);
    let session = create(&app).await;

    for content in [
        "hello",
        "燒烤我 about this fuzzy plan",
        "i am overwhelmed and exhausted",
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": content }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    let turns = turns.lock();
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].mode, Mode::PersonalAssistant);
    assert!(turns[0].system_prompt.contains("Fixture SOUL"));
    assert!(turns[0].system_prompt.contains("skill://miku-voice"));
    assert!(
        turns[0]
            .system_prompt
            .contains("skill://personal-assistant-state-capture")
    );

    assert_eq!(turns[1].mode, Mode::AmbiguityGrill);
    assert!(turns[1].system_prompt.contains("skill://ambiguity-grill"));
    assert!(
        !turns[1]
            .system_prompt
            .contains("skill://negative-state-grounding")
    );

    assert_eq!(turns[2].mode, Mode::NegativeStateGrounding);
    assert!(
        turns[2]
            .system_prompt
            .contains("skill://negative-state-grounding")
    );
    assert_eq!(turns[2].scope, "global");
}

#[tokio::test]
async fn coding_turn_prompt_uses_active_persona_bundle() {
    let temp = tempfile::tempdir().unwrap();
    write_persona_fixture(temp.path());
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let backend = Arc::new(RecordingBackend::default());
    let turns = Arc::clone(&backend.turns);
    let state = AppState::new(
        store,
        memory,
        chat,
        PersonaConfig::from_path(temp.path()),
        AuthConfig::NoAuth,
    )
    .with_coding_backend(backend);
    let app = app(state);
    let serious = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    let handoff = create_with_body(&app, Body::from(r#"{"mode":"handoff"}"#)).await;

    for (session_id, content) in [
        (serious.id, "fix the Rust code"),
        (handoff.id, "delegate this implementation handoff"),
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/sessions/{session_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": content }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    let turns = turns.lock();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].mode, Mode::SeriousEngineer);
    assert!(turns[0].system_prompt.contains("Fixture SOUL"));
    assert!(
        turns[0]
            .system_prompt
            .contains("Active mode: Serious Engineer")
    );
    assert!(!turns[0].system_prompt.contains("skill://miku-voice"));

    assert_eq!(turns[1].mode, Mode::Handoff);
    assert!(turns[1].system_prompt.contains("skill://oh-my-pi-handoff"));
    assert_eq!(turns[1].scope, "project:tempestmiku");
}
