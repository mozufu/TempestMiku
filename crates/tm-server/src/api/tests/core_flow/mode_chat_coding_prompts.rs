use super::*;

#[tokio::test]
async fn user_model_dialectic_is_third_turn_only_and_disabled_for_serious_mode() {
    let store = Arc::new(InMemoryStore::default());
    store
        .add_profile_fact(ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "boring Rust implementations".to_string(),
            confidence: 0.94,
            importance: 0.8,
            provenance: "approved-test-fixture".to_string(),
            valid_from: Utc::now(),
            valid_to: None,
        })
        .await
        .unwrap();
    let memory = Arc::new(StoreMemoryProvider::new(Arc::clone(&store)));
    let chat = Arc::new(RecordingChatRunner::default());
    let turns = Arc::clone(&chat.turns);
    let backend = Arc::new(RecordingBackend::default());
    let coding_turns = Arc::clone(&backend.turns);
    let state = AppState::new(
        Arc::clone(&store),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_coding_backend(backend);
    let app = app(state);

    let general = create(&app).await;
    for content in [
        "first question",
        "second question",
        "choose an implementation",
    ] {
        post_user_message(&app, general.id, content).await;
    }
    let serious = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    for content in ["first check", "second check", "third engineering check"] {
        post_user_message(&app, serious.id, content).await;
    }

    let turns = turns.lock();
    assert_eq!(
        turns.len(),
        3,
        "only the three general turns run through ChatRunner"
    );
    assert!(turns[0].dialectic.is_none());
    assert!(turns[1].dialectic.is_none());
    let crate::DialecticTurn::Generate(request) = turns[2]
        .dialectic
        .as_ref()
        .expect("third general turn plans dialectic")
    else {
        panic!("fresh third turn must generate, not reuse, a dialectic");
    };
    assert_eq!(request.turn_number, 3);
    assert_eq!(request.max_chars, tm_memory::DEFAULT_DIALECTIC_MAX_CHARS);
    assert_eq!(request.facts.len(), 1);
    assert!(
        request.facts[0]
            .text
            .contains("boring Rust implementations")
    );
    assert!(
        request.facts[0]
            .source_uri
            .starts_with("memory://profile/brian/facts/")
    );
    // Serious/engineering turns dispatch to the coding backend, whose turn payload has no
    // user-model dialectic concept at all: the auxiliary pass is structurally excluded from
    // coding mode, not merely left unset on a chat turn.
    let coding_turns = coding_turns.lock();
    assert_eq!(
        coding_turns.len(),
        3,
        "the three serious turns run through the coding backend"
    );
    assert!(
        coding_turns
            .iter()
            .all(|turn| turn.mode == ModeId::from("serious_engineer"))
    );
}

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
    assert!(turns[0].capabilities.iter().any(|cap| cap == "drive.*"));
    for retired in ["memory.recall", "memory.propose"] {
        assert!(
            !turns[0].capabilities.iter().any(|cap| cap == retired),
            "general turn unexpectedly received retired {retired}"
        );
    }
    for denied in [
        "fs.*",
        "code.*",
        "proc.*",
        "resources.read:linked",
        "agents.*",
        "resources.read:agent",
        "resources.read:history",
    ] {
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
    post_user_message(&app, serious.id, "delegate this implementation handoff").await;

    let turns = turns.lock();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].mode, ModeId::from("serious_engineer"));
    assert!(turns[0].system_prompt.contains("Fixture SOUL"));
    assert!(
        turns[0]
            .system_prompt
            .contains("serious-engineer-ops fixture body")
    );
    assert!(!turns[0].system_prompt.contains("miku-voice fixture body"));
    for capability in [
        "fs.*",
        "code.*",
        "proc.*",
        "resources.read:linked",
        "agents.*",
        "resources.read:agent",
        "resources.read:history",
        "backend.coding",
    ] {
        assert!(
            turns[0].capabilities.iter().any(|cap| cap == capability),
            "serious engineer turn must declare {capability}"
        );
    }
}
