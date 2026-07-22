use super::super::*;

#[tokio::test]
async fn serious_engineer_persists_open_loops_without_automatic_next_session_recall() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let backend = Arc::new(RecordingBackend::default());
    let turns = Arc::clone(&backend.turns);
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_coding_backend(backend);
    let app = app(state);
    let session_a = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    assert_eq!(session_a.project_id, None);
    assert_eq!(session_a.memory_policy, crate::MemoryPolicy::Global);
    assert_eq!(session_a.active_skills, vec!["serious-engineer-ops"]);
    post_user_message(&app, session_a.id, "tempestmiku code open loop").await;
    let chunks = store
        .recall_chunks("global", "Project summary", 5)
        .await
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert!(
        chunks[0]
            .text
            .contains("Project summary/open loop from session")
    );

    let session_b = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    post_user_message(&app, session_b.id, "tempestmiku code open loop").await;
    let turns = turns.lock();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].user_prompt, "tempestmiku code open loop");
    assert!(!turns[1].user_prompt.contains("Project summary/open loop"));
    assert!(
        turns[1]
            .capabilities
            .iter()
            .all(|capability| capability != MEMORY_SEARCH_CAPABILITY)
    );
}

#[tokio::test]
async fn serious_engineer_uses_coding_backend_and_replays_events() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_coding_backend(Arc::new(ScriptedBackend::events()));
    let (app, store) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    post_user_message(&app, session.id, "ship p0a production code").await;

    let events = store.events_after(session.id, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["mode", "text", "diff", "artifact", "final"]
    );
    let replay = store.events_after(session.id, Some(1)).await.unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["text", "diff", "artifact", "final"]
    );
    let final_event = events
        .iter()
        .find(|event| event.event_type == "final")
        .unwrap();
    let final_text = final_event.payload_json.to_string();
    assert!(final_text.contains("artifact://"));
    assert!(!final_text.contains("喵"));
}

#[tokio::test]
async fn approval_route_resolves_pending_backend_permission() {
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    );
    let broker = Arc::clone(&state.approval_broker);
    state = state.with_coding_backend(Arc::new(ScriptedBackend::approval(Arc::clone(&broker))));
    let (app, store) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/messages", session.id))
                .header("content-type", "application/json")
                .body(message_body("needs approval for a code patch"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;

    let approval_id = wait_for_event_payload(&store, session.id, "approval").await["approvalId"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{}/approvals/{}",
                    session.id, approval_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"decision":"approve"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        wait_for_turn(&app, session.id, turn_id).await["status"],
        json!("completed")
    );

    let resolved = store
        .events_after(session.id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "approval_resolved")
        .unwrap();
    assert_eq!(resolved.payload_json["outcome"], json!("selected"));
    assert_eq!(resolved.payload_json["optionId"], json!("allow"));
}
