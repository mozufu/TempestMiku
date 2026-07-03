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
