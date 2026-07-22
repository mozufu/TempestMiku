use super::*;

struct SearchMemoryChat {
    llm: Arc<ScriptedLlm>,
    temp: tempfile::TempDir,
}

impl SearchMemoryChat {
    fn new(searches: usize) -> Self {
        let mut scripts = Vec::with_capacity(searches * 2);
        for index in 0..searches {
            scripts.push(vec![
                StreamEvent::ToolCall {
                    index: 0,
                    id: Some(format!("memory-search-{index}")),
                    name: Some("execute".to_string()),
                    arguments: Some(
                        json!({"code": "@memory.search {query: \"scope marker\"}"}).to_string(),
                    ),
                },
                StreamEvent::Finish {
                    reason: Some("tool_calls".to_string()),
                },
            ]);
            scripts.push(vec![
                StreamEvent::Text("memory searched".to_string()),
                StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                },
            ]);
        }
        Self {
            llm: Arc::new(ScriptedLlm::new(scripts)),
            temp: tempfile::tempdir().unwrap(),
        }
    }

    fn runner(&self) -> AgentChatRunner {
        AgentChatRunner::tm(
            self.llm.clone(),
            AgentConfig {
                model: "fake".to_string(),
                max_turns: 3,
                ..AgentConfig::default()
            },
            TmSandboxOptions {
                artifact_root: self.temp.path().join("artifacts"),
                ..TmSandboxOptions::default()
            },
        )
    }
}

async fn search_scope(app: &Router, store: &InMemoryStore, session_id: Uuid) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(message_body("search the scope marker"))
                .unwrap(),
        )
        .await
        .unwrap();
    let turn_id = accepted_turn_id(response).await;
    let completed = wait_for_turn(app, session_id, turn_id).await;
    assert_eq!(completed["status"], json!("completed"));
    store
        .event_for_turn(session_id, turn_id, "memory_recall")
        .await
        .unwrap()
        .expect("memory search event")
        .payload_json["context"]
        .clone()
}

#[tokio::test]
async fn project_authority_and_memory_policy_are_independent() {
    let store = Arc::new(InMemoryStore::default());
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "global scope marker".to_string(),
            source: "memory://fixture/global".to_string(),
            importance: 0.9,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    store
        .add_recall_chunk(RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "project:tempestmiku".to_string(),
            text: "project scope marker".to_string(),
            source: "memory://fixture/project".to_string(),
            importance: 0.9,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let scripted = SearchMemoryChat::new(3);
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: std::env::current_dir().unwrap(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let state = AppState::new(
        store.clone(),
        Arc::new(StoreMemoryProvider::new(store.clone())),
        Arc::new(scripted.runner()),
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked);
    let app = app(state);

    let session = create_with_body(
        &app,
        Body::from(r#"{"projectId":"tempestmiku","memoryPolicy":"global"}"#),
    )
    .await;
    let overview = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/project", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overview.status(), StatusCode::OK);

    let global = search_scope(&app, store.as_ref(), session.id).await;
    assert_eq!(global["scope"], json!("global"));
    assert_eq!(
        global["recallChunks"][0]["text"],
        json!("global scope marker (importance 0.90)")
    );

    let changed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{}/scope", session.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"projectId":"tempestmiku","memoryPolicy":"project"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    let project = search_scope(&app, store.as_ref(), session.id).await;
    assert_eq!(project["scope"], json!("project:tempestmiku"));
    assert_eq!(
        project["recallChunks"][0]["text"],
        json!("project scope marker (importance 0.90)")
    );

    let invalid = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"memoryPolicy":"project"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let global_session = create(&app).await;
    let archived = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/projects/tempestmiku/archive")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reason":"verification"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/project", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    let unaffected = search_scope(&app, store.as_ref(), global_session.id).await;
    assert_eq!(unaffected["scope"], json!("global"));
    assert_eq!(
        unaffected["recallChunks"][0]["text"],
        json!("global scope marker (importance 0.90)")
    );
}
