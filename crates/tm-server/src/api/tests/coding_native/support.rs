use super::super::*;

pub(super) async fn native_tm_approval_app(
    timeout: Duration,
    code: &str,
) -> (
    Router,
    Arc<InMemoryStore>,
    Arc<ScriptedLlm>,
    CreateSessionResponse,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(linked_root.join("src")).unwrap();
    std::fs::write(
        linked_root.join("Cargo.toml"),
        "[package]\nname = \"native-tm-approval-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(linked_root.join("src/lib.rs"), "pub fn x() -> i32 { 1 }\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();
    let tool_args = json!({ "code": code }).to_string();
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_native_tm".to_string()),
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
            artifact_root,
            linked_folders: Some(linked),
            approval_timeout: timeout,
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
    (router, store, llm, session, temp)
}

pub(super) async fn native_tm_effect_approval_app(
    timeout: Duration,
) -> (
    Router,
    Arc<InMemoryStore>,
    Arc<ScriptedLlm>,
    CreateSessionResponse,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("remove-me.txt"), "delete-me\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let code = r#"
let hits = @code.search {pattern: "delete-me", paths: ["repo:remove-me.txt"], regex: false};
hits |> par map (fun hit -> @fs.remove {path: hit.path, tag: hit.tag}) |> display {kind: "table"}
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_native_tm".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({ "code": code }).to_string()),
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
            artifact_root,
            linked_folders: Some(linked),
            approval_timeout: timeout,
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
    (router, store, llm, session, temp)
}

pub(super) async fn native_tm_simple_app(
    code: &str,
    cancellation: Option<Arc<dyn tm_core::CancellationToken>>,
) -> (
    Router,
    Arc<InMemoryStore>,
    Arc<ScriptedLlm>,
    CreateSessionResponse,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let llm = Arc::new(ScriptedLlm::new(vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_native_tm_simple".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(json!({ "code": code }).to_string()),
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
    let llm_client: Arc<dyn LlmClient> = llm.clone();
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
    let backend = NativeTmBackend::new(
        llm_client,
        AgentConfig {
            model: "fake".into(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root,
            cancellation,
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Deny,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let session = create_with_body(&router, Body::from(r#"{"mode":"serious_engineer"}"#)).await;
    (router, store, llm, session, temp)
}

pub(super) fn native_proc_script() -> &'static str {
    r#"
let result = handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error {
  | ApprovalDeniedError {message, ...} -> {ok: false, name: "ApprovalDeniedError", message: message}
  | ApprovalTimeoutError {message, ...} -> {ok: false, name: "ApprovalTimeoutError", message: message}
  | other -> rethrow other
};
result |> display {kind: "json"}
"#
}

pub(super) fn native_tool_result(llm: &ScriptedLlm) -> String {
    let requests = llm.requests.lock();
    requests[1]
        .iter()
        .find(|message| message.role == Role::Tool)
        .expect("tool result is fed back before final turn")
        .content
        .clone()
}
