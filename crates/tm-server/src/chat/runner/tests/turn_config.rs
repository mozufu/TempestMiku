use super::*;

#[tokio::test]
async fn prior_messages_are_included_before_current_user_message() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let mut turn = chat_turn(Uuid::from_u128(7));
    turn.prior_messages = vec![
        Message::user("earlier question"),
        Message::assistant("earlier answer"),
    ];

    run(&runner, turn).await;

    let requests = harness.llm.requests.lock().unwrap();
    assert_eq!(requests[0][0].role, Role::System);
    assert!(
        requests[0][0]
            .content
            .starts_with("## Immutable tm runtime contract")
    );
    assert!(requests[0][0].content.contains("test system"));
    assert!(requests[0][0].content.contains("tm-conformance-v2"));
    assert_eq!(requests[0][1], Message::user("earlier question"));
    assert_eq!(requests[0][2], Message::assistant("earlier answer"));
    assert_eq!(requests[0][3], Message::user("advance state"));
}

#[tokio::test]
async fn per_turn_limits_clamp_turn_count_and_cell_wall_budget() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let mut turn = chat_turn(Uuid::from_u128(8));
    turn.limits = ChatRunLimits {
        max_turns: Some(1),
        cell_wall_ms: Some(7),
    };
    let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(NullSink);

    let error = runner.run_turn(turn, sink).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("turn budget exhausted after 1 turns")
    );
    assert_eq!(*harness.wall_budgets.lock().unwrap(), vec![7]);
    assert_eq!(harness.llm.requests.lock().unwrap().len(), 1);

    let mut cfg = AgentConfig {
        max_turns: 3,
        cell_budget: CellBudget {
            wall_ms: 30,
            ..CellBudget::default()
        },
        ..AgentConfig::default()
    };
    apply_turn_limits(
        &mut cfg,
        ChatRunLimits {
            max_turns: Some(8),
            cell_wall_ms: Some(120),
        },
    );
    assert_eq!(
        cfg.max_turns, 3,
        "a turn cannot expand the base turn budget"
    );
    assert_eq!(
        cfg.cell_budget.wall_ms, 30,
        "a turn cannot expand the base cell budget"
    );
}

#[serial_test::serial]
#[tokio::test]
async fn deny_approvals_replaces_the_base_policy() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: repo,
        mode: FsMode::Rw,
        commands: vec!["cargo".to_string()],
        safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
    }])
    .unwrap();
    let llm = Arc::new(ReactiveLlm::with_code(
        r#"let result = handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error {
  | ApprovalDeniedError {message, ...} -> {name: "ApprovalDeniedError", message: message}
  | other -> rethrow other
};
result |> display {kind: "json"}"#,
    ));
    let llm_client: Arc<dyn LlmClient> = llm.clone();
    let runner = AgentChatRunner::tm_with_options(
        llm_client,
        AgentConfig {
            model: "fake".to_string(),
            max_turns: 3,
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root: temp.path().join("artifacts"),
            linked_folders: Some(linked),
            approval_policy: Arc::new(AlwaysApprove),
            ..TmSandboxOptions::default()
        },
        options(1, Duration::from_secs(60)),
    );
    let mut turn = chat_turn(Uuid::from_u128(10));
    turn.scope = "project:repo".to_string();
    turn.capabilities = vec!["proc.*".to_string()];
    turn.deny_approvals = true;

    run(&runner, turn).await;

    let requests = llm.requests.lock().unwrap();
    let tool_result = requests[1]
        .iter()
        .find(|message| message.role == Role::Tool)
        .expect("tool result is fed back before final turn");
    assert!(
        tool_result.content.contains("ApprovalTimeoutError"),
        "tool result: {}",
        tool_result.content
    );
}
