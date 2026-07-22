use super::*;

#[tokio::test]
async fn durable_memory_search_event_is_not_automatically_injected_on_retry() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "memory-retry", "retry the durable turn")
        .await
        .unwrap();
    let memory = MemoryContext::from_records(
        "owner",
        "global",
        Vec::new(),
        vec![RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "persisted search result remains behind memory.search".to_string(),
            source: "memory://fixture/retry".to_string(),
            importance: 0.8,
            created_at: Utc::now(),
        }],
        1_600,
    )
    .mark_lexical_fallback("fixture_restart");
    store
        .append_event_for_turn(
            session.id,
            "memory_recall",
            json!({
                "schemaVersion": 1,
                "resourceUri": format!("memory://recalls/{}", turn.id),
                "context": memory,
            }),
            Some(turn.id),
        )
        .await
        .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(ReplayMustNotQueryMemory),
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(false);

    let output = execute_turn_body(&state, &turn).await.unwrap();
    assert_eq!(output.response, "Miku heard: retry the durable turn");
    assert_eq!(
        store
            .events_by_type(session.id, "memory_recall", 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn persisted_memory_recall_rechecks_a_revoked_project_scope() {
    let store = Arc::new(InMemoryStore::default());
    store.configure_owner_subject("owner").await.unwrap();
    let persona = tm_modes::ModesConfig::default();
    let assets = persona.load_assets();
    let session = store
        .create_session(NewSession {
            mode: assets.modes.default_mode(),
            persona_status: assets.status.clone(),
        })
        .await
        .unwrap();
    store
        .set_session_memory_scope(session.id, "project:tempestmiku")
        .await
        .unwrap();
    let turn = store
        .enqueue_turn(session.id, "revoked-memory-retry", "retry the durable turn")
        .await
        .unwrap();
    let memory = MemoryContext::from_records(
        "owner",
        "project:tempestmiku",
        Vec::new(),
        vec![RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:tempestmiku".to_string(),
            text: "revoked project memory must not survive retry".to_string(),
            source: "memory://fixture/revoked-retry".to_string(),
            importance: 0.8,
            created_at: Utc::now(),
        }],
        1_600,
    );
    store
        .append_event_for_turn(
            session.id,
            "memory_recall",
            json!({
                "schemaVersion": 1,
                "resourceUri": format!("memory://recalls/{}", turn.id),
                "context": memory,
            }),
            Some(turn.id),
        )
        .await
        .unwrap();
    store
        .revoke_memory_scope("owner", "project:tempestmiku", "linked folder removed")
        .await
        .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: std::env::current_dir().unwrap(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let state = AppState::new(
        Arc::clone(&store),
        Arc::new(ReplayMustNotQueryMemory),
        Arc::new(EchoChatRunner),
        persona,
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked)
    .with_auto_turn_dispatcher(false);

    let error = execute_turn_body(&state, &turn).await.unwrap_err();
    assert!(
        matches!(error, ServerError::NotFound(message) if message.contains("project:tempestmiku"))
    );
}
