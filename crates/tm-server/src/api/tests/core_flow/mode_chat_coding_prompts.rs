use super::*;

#[tokio::test]
async fn modes_catalog_is_loaded_from_runtime_mode_assets() {
    let temp = tempfile::tempdir().unwrap();
    write_mode_assets_fixture(temp.path());
    let custom = serde_json::json!({
        "defaultMode": "custom_runtime_mode",
        "modes": [
            {
                "mode": "custom_runtime_mode",
                "label": "Custom Runtime Mode",
                "description": "Runtime-only mode from the mode catalog.",
                "voiceCap": "medium",
                "defaultScope": "global",
                "activeSkills": ["miku-voice"],
                "capabilityClass": "conversation",
                "route": {
                    "isDefault": true,
                    "priority": 0,
                    "triggers": []
                }
            }
        ]
    });
    std::fs::write(
        temp.path().join("modes.json"),
        serde_json::to_string_pretty(&custom).unwrap(),
    )
    .unwrap();

    let (app, _) = test_app(ModesConfig::from_path(temp.path()), AuthConfig::NoAuth);
    let catalog = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/modes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog.status(), StatusCode::OK);
    let catalog = response_json(catalog).await;
    assert_eq!(catalog["defaultMode"], json!("custom_runtime_mode"));
    assert_eq!(catalog["modes"][0]["mode"], json!("custom_runtime_mode"));

    let session = create(&app).await;
    assert_eq!(session.mode, ModeId::from("custom_runtime_mode"));
    assert_eq!(session.label, "Custom Runtime Mode");
}

#[tokio::test]
async fn missing_mode_assets_path_boots_degraded_with_warning() {
    let (app, _) = test_app(
        ModesConfig::from_path("/definitely/missing/tempestmiku/modes"),
        AuthConfig::NoAuth,
    );
    let session = create(&app).await;
    match session.persona_status {
        crate::AssetStatus::Degraded { warning } => assert!(warning.contains("missing")),
        crate::AssetStatus::Loaded { .. } => panic!("missing path must degrade"),
    }
}

#[tokio::test]
async fn chat_turn_prompt_uses_active_mode_bundle() {
    let temp = tempfile::tempdir().unwrap();
    write_mode_assets_fixture(temp.path());
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(RecordingChatRunner::default());
    let turns = Arc::clone(&chat.turns);
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::from_path(temp.path()),
        AuthConfig::NoAuth,
    );
    let app = app(state);
    let session = create(&app).await;

    for content in [
        "hello",
        "燒烤我 about this fuzzy plan",
        "i am overwhelmed and exhausted",
    ] {
        post_user_message(&app, session.id, content).await;
    }

    let turns = turns.lock();
    assert_eq!(turns.len(), 3);
    // All three turns stay in `general` mode now: ambiguity-grill and negative-state-grounding
    // are triggered layered skills, not separate modes, so they never change turns[*].mode.
    assert_eq!(turns[0].mode, ModeId::from("general"));
    assert!(
        turns[0]
            .capabilities
            .iter()
            .any(|cap| cap == "memory.recall")
    );
    for denied in ["fs.*", "code.*", "proc.*", "resources.read:linked"] {
        assert!(
            !turns[0].capabilities.iter().any(|cap| cap == denied),
            "general turn unexpectedly received {denied}"
        );
    }
    assert!(turns[0].system_prompt.contains("Fixture SOUL"));
    assert!(turns[0].system_prompt.contains("miku-voice fixture body"));
    assert!(
        turns[0]
            .system_prompt
            .contains("personal-assistant-state-capture fixture body")
    );
    // scope-guard is always-on: it composes into every turn regardless of message content.
    assert!(turns[0].system_prompt.contains("scope-guard fixture body"));
    assert!(
        !turns[0]
            .system_prompt
            .contains("ambiguity-grill fixture body")
    );
    assert!(
        !turns[0]
            .system_prompt
            .contains("negative-state-grounding fixture body")
    );

    assert_eq!(turns[1].mode, ModeId::from("general"));
    assert!(
        turns[1]
            .system_prompt
            .contains("ambiguity-grill fixture body")
    );
    assert!(turns[1].system_prompt.contains("scope-guard fixture body"));
    assert!(
        !turns[1]
            .system_prompt
            .contains("negative-state-grounding fixture body")
    );

    assert_eq!(turns[2].mode, ModeId::from("general"));
    assert!(
        turns[2]
            .system_prompt
            .contains("negative-state-grounding fixture body")
    );
    assert!(
        !turns[2]
            .system_prompt
            .contains("ambiguity-grill fixture body")
    );
    assert_eq!(turns[2].scope, "global");
}

#[tokio::test]
async fn coding_turn_prompt_uses_active_mode_bundle() {
    let temp = tempfile::tempdir().unwrap();
    write_mode_assets_fixture(temp.path());
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let backend = Arc::new(RecordingBackend::default());
    let turns = Arc::clone(&backend.turns);
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::from_path(temp.path()),
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
        post_user_message(&app, session_id, content).await;
    }

    let turns = turns.lock();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].mode, ModeId::from("serious_engineer"));
    assert!(turns[0].system_prompt.contains("Fixture SOUL"));
    assert!(
        turns[0]
            .system_prompt
            .contains("serious-engineer-ops fixture body")
    );
    assert!(!turns[0].system_prompt.contains("miku-voice fixture body"));
    for capability in ["fs.*", "code.*", "proc.*", "resources.read:linked"] {
        assert!(turns[0].capabilities.iter().any(|cap| cap == capability));
    }

    assert_eq!(turns[1].mode, ModeId::from("handoff"));
    assert!(
        turns[1]
            .system_prompt
            .contains("oh-my-pi-handoff fixture body")
    );
    assert!(
        !turns[1].system_prompt.contains("miku-voice fixture body"),
        "handoff mode must not inject miku-voice skill"
    );
    assert_eq!(turns[1].scope, "global");
    assert!(
        turns[1].capabilities.iter().any(|c| c == "agents.*"),
        "handoff mode must declare agents.* capability"
    );
    for denied in ["fs.*", "code.*", "proc.*", "resources.read:linked"] {
        assert!(
            !turns[1].capabilities.iter().any(|cap| cap == denied),
            "handoff turn unexpectedly received {denied}"
        );
    }
}
