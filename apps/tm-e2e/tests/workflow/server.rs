use super::fakes::*;
use super::support::*;
use super::*;

pub(super) async fn start_server(
    auth: AuthConfig,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(store, memory, chat, tm_server::ModesConfig::default(), auth)
        .with_auto_turn_dispatcher(true)
        .with_linked_folders(test_linked_project(temp.path()))
        .with_artifact_root(artifact_root.clone())
        .with_coding_backend(Arc::new(ArtifactBackend {
            root: artifact_root,
        }));
    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

pub(super) async fn start_native_actor_coordination_server()
-> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let parent_code = native_parent_coordination_code();
    let child_code = native_child_coordination_code();
    let llm = Arc::new(ScriptedLlm::new(vec![
        execute_script("parent_call", &parent_code),
        execute_script("child_call_0", &child_code),
        execute_script("child_call_1", &child_code),
        text_script(NATIVE_P3_FINAL_TEXT),
        text_script(NATIVE_P3_FINAL_TEXT),
        text_script(NATIVE_P3_FINAL_TEXT),
        text_script(NATIVE_P3_FINAL_TEXT),
    ]));
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 6,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };

    let roster = Arc::new(MailboxRegistry::new());
    let mut sandbox_options = TmSandboxOptions {
        artifact_root: artifact_root.clone(),
        approval_timeout: Duration::from_secs(5),
        ..TmSandboxOptions::default()
    };
    tm_agents::register(
        &mut sandbox_options.host_registry,
        &mut sandbox_options.resource_registry,
        Arc::clone(&roster),
    );

    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store,
        memory,
        chat,
        tm_server::ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(true)
    .with_artifact_root(artifact_root.clone())
    .with_actor_roster(Arc::clone(&roster));
    let broker = Arc::clone(&state.approval_broker);

    let executor_options = sandbox_options.clone();
    let executor_roster = Arc::clone(&roster);
    let executor_approval_roster = Arc::clone(&roster);
    let executor_broker = Arc::clone(&broker);
    let llm_for_executor: Arc<dyn LlmClient> = llm.clone();
    let executor: Arc<dyn tm_agents::ActorExecutor> =
        Arc::new(ChatActorExecutor::with_actor_context(
            llm_for_executor,
            cfg.clone(),
            move |session_id, actor_id, grants, session_scope, cancellation| {
                let mut opts = executor_options.clone();
                opts.session_id = session_id.to_string();
                opts.actor_id = actor_id.map(str::to_string);
                opts.session_scope = session_scope.map(str::to_string);
                opts.cancellation = cancellation;
                opts.grants =
                    CapabilityGrants::default().allow_many(grants.names().map(str::to_string));
                let sink: Arc<dyn CodingEventSink> = Arc::new(RosterCodingEventSink::new(
                    session_id,
                    Arc::clone(&executor_approval_roster),
                ));
                opts.approval_policy = Arc::new(
                    HttpApprovalPolicy::new(Arc::clone(&executor_broker), session_id, sink)
                        .with_actor_id(actor_id.map(str::to_string)),
                );
                Arc::new(TmSandbox::new(opts)) as Arc<dyn tm_core::Sandbox>
            },
            Some(artifact_root.clone()),
            executor_roster,
        ));
    roster.set_executor(executor);

    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let backend = NativeTmBackend::new(
        llm_for_backend,
        cfg,
        sandbox_options,
        NativeApprovalMode::Manual,
        broker,
    );
    state = state.with_coding_backend(Arc::new(backend));
    state.wire_lifecycle_sink();

    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

pub(super) async fn start_native_tm_server()
-> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("remove-me.txt"), "delete-me\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let code = r#"
let hits = @fs.grep {pattern: "delete-me", paths: ["repo:remove-me.txt"], regex: false};
hits |> par map (fun hit -> @fs.remove {path: hit.path, tag: hit.tag}) |> display {kind: "table"}
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        execute_script("tm_e2e_edit", code),
        text_script("tm e2e complete"),
    ]));
    let llm_client: Arc<dyn LlmClient> = llm;
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store,
        memory,
        chat,
        tm_server::ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(true)
    .with_artifact_root(artifact_root.clone())
    .with_linked_folders(linked.clone());
    let backend = NativeTmBackend::new(
        llm_client,
        AgentConfig {
            model: "fake".into(),
            max_turns: 3,
            cell_budget: CellBudget {
                wall_ms: 30_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            approval_timeout: Duration::from_secs(5),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

pub(super) async fn start_drive_smoke_server()
-> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let drive_store = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(temp.path(), "drive").unwrap(),
    );
    let llm = Arc::new(ScriptedLlm::new(vec![
        execute_script("drive_smoke_call", &drive_smoke_code()),
        text_script("drive smoke complete"),
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

    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store,
        memory,
        chat,
        tm_server::ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(true)
    .with_linked_folders(test_linked_project(temp.path()))
    .with_artifact_root(artifact_root.clone())
    .with_drive_store(drive_store.clone());
    let broker = Arc::clone(&state.approval_broker);
    let backend = NativeTmBackend::new(
        llm,
        cfg,
        TmSandboxOptions {
            artifact_root,
            drive_store: Some(Arc::new(drive_store)),
            approval_timeout: Duration::from_secs(5),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        broker,
    );
    state = state.with_coding_backend(Arc::new(backend));

    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

pub(super) async fn start_actor_smoke_server()
-> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let roster = Arc::new(MailboxRegistry::new());
    let mut state = AppState::new(
        store,
        memory,
        chat,
        tm_server::ModesConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_auto_turn_dispatcher(true)
    .with_artifact_root(artifact_root.clone())
    .with_actor_roster(Arc::clone(&roster));
    let broker = Arc::clone(&state.approval_broker);
    state = state.with_coding_backend(Arc::new(ActorSmokeBackend {
        root: artifact_root,
        broker,
        roster,
    }));
    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}
