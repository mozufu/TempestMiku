use super::super::*;
use super::support::native_tool_result;

#[tokio::test]
async fn artifact_resource_route_reads_session_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(root.clone())
    .with_coding_backend(Arc::new(ScriptedBackend::artifact(root)));
    let (app, _) = test_app_with_state(state);
    let session = create_with_body(&app, Body::from(r#"{"mode":"serious_engineer"}"#)).await;

    post_user_message(&app, session.id, "write code transcript").await;

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = response_json(list).await;
    assert_eq!(list_json[0]["uri"], json!("artifact://0"));

    let read = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let read_json = response_json(read).await;
    assert_eq!(read_json["uri"], json!("artifact://0"));
    assert!(
        read_json["content"]
            .as_str()
            .unwrap()
            .contains("scripted transcript")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn general_and_handoff_turns_deny_undeclared_linked_repo_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[workspace]\nmembers = []\n",
    )
    .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();

    let code = r#"
@fs.read {path: "repo:Cargo.toml"}
"#;
    let tool_turn = || {
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_denied_linked".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({ "code": code }).to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ]
    };
    let final_turn = || {
        vec![
            StreamEvent::Text("denials checked".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ]
    };
    let llm = Arc::new(ScriptedLlm::new(vec![
        tool_turn(),
        final_turn(),
        tool_turn(),
        final_turn(),
    ]));
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        ..AgentConfig::default()
    };
    let chat = Arc::new(AgentChatRunner::tm(
        llm.clone(),
        cfg.clone(),
        TmSandboxOptions {
            artifact_root: artifact_root.clone(),
            linked_folders: Some(linked.clone()),
            ..TmSandboxOptions::default()
        },
    ));
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let mut state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_linked_folders(linked.clone());
    let backend = NativeTmBackend::new(
        llm_for_backend,
        cfg,
        TmSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);

    for mode in ["general", "handoff"] {
        let session =
            create_with_body(&router, Body::from(json!({ "mode": mode }).to_string())).await;
        post_user_message(&router, session.id, "inspect the linked project").await;
    }

    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 4);
    for request_index in [1, 3] {
        let tool_result = requests[request_index]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn");
        assert!(
            tool_result.content.contains("unknown capability fs.read"),
            "tool result: {}",
            tool_result.content
        );
    }
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn sensitive_cell_boundary_server_route_uses_structured_events_and_preserves_denials() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[workspace]\nmembers = []\n",
    )
    .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();

    let code = r#"
let doc = @fs.read {path: "repo:Cargo.toml"};
let artifact = @artifacts.put {data: "server sdk artifact\n#{doc.content}", title: "server-sdk", mime: "text/plain"};
let unsafeProc = handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error {
  | ApprovalTimeoutError {message, ...} -> {name: "ApprovalTimeoutError", message: message}
  | other -> rethrow other
};
let deniedHttp = handle (@http.request {method: "GET", url: "https://evil.test/"}) with error {
  | CapabilityDeniedError {message, ...} -> {name: "CapabilityDeniedError", message: message}
  | other -> rethrow other
};
{
  ok: contains "[workspace]" doc.content,
  artifact: artifact.uri,
  unsafeProc,
  deniedHttp
} |> display {kind: "json"}
"#;
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_server_sdk".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("done".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let chat = Arc::new(EchoChatRunner);
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let mut state = AppState::new(
        store.clone(),
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone());
    let backend = NativeTmBackend::new(
        llm_for_backend,
        cfg,
        TmSandboxOptions {
            artifact_root: artifact_root.clone(),
            linked_folders: Some(linked),
            approval_timeout: Duration::from_millis(1),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:repo"}"#),
    )
    .await;

    post_user_message(&router, session.id, "exercise the server sdk").await;

    let tool_result_content = {
        let requests = llm.requests.lock();
        assert_eq!(requests.len(), 2);
        requests[1]
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result is fed back before final turn")
            .content
            .clone()
    };
    assert!(
        tool_result_content.contains("\"ok\": true"),
        "tool result: {tool_result_content}"
    );
    assert!(
        tool_result_content.contains("artifact://0"),
        "tool result: {tool_result_content}"
    );
    assert!(
        tool_result_content.contains("ApprovalTimeoutError"),
        "tool result: {tool_result_content}"
    );
    assert!(
        tool_result_content.contains("CapabilityDeniedError"),
        "tool result: {tool_result_content}"
    );

    let events = store.events_after(session.id, None).await.unwrap();
    let cell_starts = events
        .iter()
        .filter(|event| event.event_type == "cell_start")
        .collect::<Vec<_>>();
    let cell_results = events
        .iter()
        .filter(|event| event.event_type == "cell_result")
        .collect::<Vec<_>>();
    assert_eq!(cell_starts.len(), 1, "{cell_starts:#?}");
    assert_eq!(cell_results.len(), 1, "{cell_results:#?}");
    assert_eq!(cell_starts[0].payload_json["sourcePreview"], "[redacted]");
    assert_eq!(cell_results[0].payload_json["resultPreview"], "[redacted]");
    assert!(cell_starts[0].payload_json.get("code").is_none());
    assert!(cell_results[0].payload_json.get("shaped").is_none());
    let persisted = serde_json::to_string(&events).unwrap();
    assert!(!persisted.contains("https://evil.test/"), "{persisted}");
    assert!(events.iter().any(
        |event| event.event_type == "final" && event.payload_json.to_string().contains("done")
    ));

    let list = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = response_json(list).await;
    assert_eq!(list_json[0]["uri"], json!("artifact://0"));

    let read = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sessions/{}/resources/artifacts/0", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let read_json = response_json(read).await;
    assert!(
        read_json["content"]
            .as_str()
            .unwrap()
            .contains("server sdk artifact")
    );
}

#[serial_test::serial]
#[tokio::test(flavor = "current_thread")]
async fn serious_engineer_native_tm_uses_linked_repo_and_scoped_drive_search() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(linked_root.join("src")).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[package]\nname = \"linked-drive-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        linked_root.join("src/lib.rs"),
        "pub fn linked_answer() -> i32 { 42 }\n",
    )
    .unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();
    let drive_store = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(temp.path(), "drive").unwrap(),
    );
    let filed = drive_store
        .put_bytes(
            b"# Scoped Linked Brief\nScoped drive metadata for the linked project.",
            tm_drive::DrivePutOptions {
                auto: true,
                suggested_path: Some("projects/repo/notes/scoped-linked.md".to_string()),
                project: Some("repo".to_string()),
                source_uri: Some("linked://repo/README.md".to_string()),
                approval_mode: tm_drive::DriveApprovalMode::Auto,
                ..tm_drive::DrivePutOptions::default()
            },
        )
        .unwrap();

    let code = format!(
        r#"
let read = @fs.read {{path: "repo:src/lib.rs"}};
let hits = @fs.grep {{pattern: "linked_answer", paths: ["repo:src/lib.rs"], regex: false}};
let driveHits = @drive.search {{query: "Scoped", project: "repo", returnSnippets: true}};
let driveHit = match driveHits {{ | first :: _ -> first | [] -> null }};
{{
  readOk: contains "linked_answer" read.content,
  codeHits: length hits,
  driveHits: length driveHits,
  driveUri: driveHit.uri,
  driveProject: driveHit.project,
  expectedDriveUri: "{uri}"
}} |> display {{kind: "json"}}
"#,
        uri = filed.uri
    );
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_linked_drive".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(tool_args),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::Text("linked project checked".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ],
    ]));
    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store,
        memory,
        chat,
        ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone())
    .with_drive_store(drive_store.clone());
    let backend = NativeTmBackend::new(
        llm_for_backend,
        cfg,
        TmSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            drive_store: Some(Arc::new(drive_store)),
            approval_timeout: Duration::from_secs(1),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(
        &router,
        Body::from(r#"{"mode":"serious_engineer","scope":"project:repo"}"#),
    )
    .await;

    post_user_message(&router, session.id, "inspect the linked project").await;

    let tool_result = native_tool_result(&llm);
    assert!(tool_result.contains("\"readOk\": true"), "{tool_result}");
    assert!(tool_result.contains("\"codeHits\": 1"), "{tool_result}");
    assert!(tool_result.contains("\"driveHits\": 1"), "{tool_result}");
    assert!(tool_result.contains(&filed.uri), "{tool_result}");
    assert!(
        tool_result.contains("\"driveProject\": \"repo\""),
        "{tool_result}"
    );
}
