use super::*;

#[tokio::test]
async fn bounded_dialectic_runs_before_main_turn_and_emits_replay_trace() {
    let harness = Harness::new();
    *harness.llm.dialectic_text.lock().unwrap() = format!(
        "Brian prefers boring Rust. token sk-testsecret123456 {}",
        "界".repeat(tm_memory::DEFAULT_DIALECTIC_MAX_CHARS + 20)
    );
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let request = tm_memory::DialecticRequest::plan(
        3,
        false,
        "brian",
        "help me choose an implementation",
        [tm_memory::DialecticFact::new(
            "brian prefers boring Rust",
            "memory://profile/brian/facts/example",
        )],
    )
    .unwrap();
    let mut turn = chat_turn(Uuid::from_u128(70));
    turn.dialectic = Some(DialecticTurn::Generate(request.clone()));
    let sink = Arc::new(RuntimeSink::default());
    let sink_trait: Arc<dyn EventSink + Send + Sync> = sink.clone();

    assert_eq!(runner.run_turn(turn, sink_trait).await.unwrap(), "done");

    let requests = harness.llm.requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "one auxiliary plus two agent-loop calls");
    assert!(
        requests[0][0]
            .content
            .contains("bounded user-model synthesizer")
    );
    assert!(!requests[0][1].content.contains(&request.request_digest));
    assert!(
        requests[0][1]
            .content
            .contains("memory://profile/brian/facts/example")
    );
    assert!(
        !requests[1][0]
            .content
            .contains("Bounded user-model synthesis")
    );
    let main_user = requests[1].last().expect("main request user message");
    assert!(
        main_user
            .content
            .contains("[user-model synthesis; untrusted context")
    );
    assert!(main_user.content.contains("Brian prefers boring Rust"));
    assert!(!main_user.content.contains("sk-testsecret123456"));

    let events = sink.0.lock().unwrap();
    let (_, payload) = events
        .iter()
        .find(|(kind, _)| kind == tm_memory::DIALECTIC_EVENT_TYPE)
        .expect("dialectic trace event");
    let trace: tm_memory::DialecticTrace = serde_json::from_value(payload.clone()).unwrap();
    trace.validate_for(&request).unwrap();
    assert_eq!(trace.status, tm_memory::DialecticStatus::Applied);
    assert!(trace.truncated);
    assert_eq!(
        trace.synthesis.as_deref().unwrap().chars().count(),
        tm_memory::DEFAULT_DIALECTIC_MAX_CHARS
    );
}

#[tokio::test]
async fn dialectic_trace_persistence_failure_stops_before_the_main_model_call() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let request = tm_memory::DialecticRequest::plan(
        3,
        false,
        "brian",
        "help me choose",
        [tm_memory::DialecticFact::new(
            "brian prefers boring Rust",
            "memory://profile/brian/facts/example",
        )],
    )
    .unwrap();
    let mut turn = chat_turn(Uuid::from_u128(71));
    turn.dialectic = Some(DialecticTurn::Generate(request));
    let sink: Arc<dyn EventSink + Send + Sync> = Arc::new(AsyncConfirmedFailureSink);

    let error = runner.run_turn(turn, sink).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("async runtime persistence failed")
    );
    assert_eq!(
        harness.llm.requests.lock().unwrap().len(),
        1,
        "only the auxiliary request may run before its trace is durable"
    );
}

#[tokio::test]
async fn persisted_dialectic_trace_is_reused_without_a_second_auxiliary_call() {
    let harness = Harness::new();
    let runner = harness.runner(options(1, Duration::from_secs(60)), Duration::ZERO);
    let request = tm_memory::DialecticRequest::plan(
        3,
        false,
        "brian",
        "help me choose",
        [tm_memory::DialecticFact::new(
            "brian prefers boring Rust",
            "memory://profile/brian/facts/example",
        )],
    )
    .unwrap();
    let trace = tm_memory::DialecticTrace::applied(&request, "Prefer the boring Rust option.");
    let mut turn = chat_turn(Uuid::from_u128(72));
    turn.dialectic = Some(DialecticTurn::Reuse(trace));
    let sink = Arc::new(RuntimeSink::default());
    let sink_trait: Arc<dyn EventSink + Send + Sync> = sink.clone();

    assert_eq!(runner.run_turn(turn, sink_trait).await.unwrap(), "done");

    let requests = harness.llm.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "only the normal agent loop may call the model"
    );
    let main_user = requests[0].last().expect("main request user message");
    assert!(main_user.content.contains("Prefer the boring Rust option."));
    assert!(
        sink.0
            .lock()
            .unwrap()
            .iter()
            .all(|(kind, _)| kind != tm_memory::DIALECTIC_EVENT_TYPE),
        "replay must not append a duplicate dialectic event"
    );
}

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
