use std::path::PathBuf;

use parking_lot::Mutex;
use serde_json::Value;
use tm_core::{CellBudget, EventSink, Protocol, SessionConfig};
use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders, P0HostConfig};

use super::{
    cli::Args,
    runtime::{
        build_agent_config, build_sandbox, serious_engineer_grants, serious_engineer_prompt,
    },
    sink::StreamingSink,
};

#[test]
fn streaming_sink_writes_token_deltas_before_final_newline() {
    let sink = StreamingSink::new(Vec::new(), Vec::new(), false);

    sink.on_text("The answer ");
    sink.on_text("streams");
    assert_eq!(
        String::from_utf8(sink.stdout.lock().clone()).unwrap(),
        "The answer streams"
    );

    sink.on_final("The answer streams");
    assert_eq!(
        String::from_utf8(sink.stdout.lock().clone()).unwrap(),
        "The answer streams\n"
    );
}

#[test]
fn streaming_sink_keeps_cell_telemetry_off_stdout() {
    let sink = StreamingSink::new(Vec::new(), Vec::new(), false);

    sink.on_text("visible");
    sink.on_tool_call("execute");
    sink.on_cell_start("display(1)");
    sink.on_cell_result("stdout:\n1");

    assert_eq!(
        String::from_utf8(sink.stdout.lock().clone()).unwrap(),
        "visible"
    );
    let stderr = String::from_utf8(sink.stderr.lock().clone()).unwrap();
    assert!(stderr.contains("tool call: execute"));
    assert!(stderr.contains("executing cell"));
    assert!(stderr.contains("result"));
}

#[test]
fn args_parse_p0_config_and_session_flags() {
    let args = Args::parse(
        [
            "--config",
            ".tempestmiku/config.json",
            "--mcp-config",
            ".tempestmiku/mcp.json",
            "--session-id",
            "smoke",
            "--event-log",
            "/tmp/tm-events.jsonl",
            "--turn-budget-ok",
            "--max-turns",
            "3",
            "hello",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .unwrap();
    assert_eq!(args.config, Some(PathBuf::from(".tempestmiku/config.json")));
    assert_eq!(
        args.mcp_config,
        Some(PathBuf::from(".tempestmiku/mcp.json"))
    );
    assert_eq!(args.session_id, Some("smoke".to_string()));
    assert_eq!(args.event_log, Some(PathBuf::from("/tmp/tm-events.jsonl")));
    assert!(args.turn_budget_ok);
    assert_eq!(args.max_turns, Some(3));
    assert_eq!(args.prompt, Some("hello".to_string()));
}

#[test]
fn streaming_sink_writes_ordered_jsonl_events() {
    let event_log = tempfile::NamedTempFile::new().unwrap();
    let writer = event_log.reopen().unwrap();
    let mut sink = StreamingSink::new(Vec::new(), Vec::new(), false);
    sink.event_log = Some(Mutex::new(Box::new(writer)));

    sink.try_on_reasoning("think").unwrap();
    sink.try_on_text("working").unwrap();
    sink.try_on_tool_call("execute").unwrap();
    sink.try_on_cell_start("let one = 1").unwrap();
    sink.try_on_tool_call("execute").unwrap();
    sink.try_on_cell_start("let two = one + 1").unwrap();
    sink.try_on_cell_result("{\"result\":1}").unwrap();
    sink.try_on_cell_result("{\"result\":2}").unwrap();
    sink.try_on_turn_end().unwrap();
    sink.try_on_final("done").unwrap();

    let events = std::fs::read_to_string(event_log.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "reasoning_delta",
            "text_delta",
            "tool_call",
            "cell_start",
            "tool_call",
            "cell_start",
            "cell_result",
            "cell_result",
            "turn_end",
            "final",
        ]
    );
    assert_eq!(events[3]["code"], "let one = 1");
    assert_eq!(events[7]["result"], "{\"result\":2}");
}

#[test]
fn args_reject_removed_sandbox_flags() {
    for flag in ["--stub-sandbox", "--tm-sandbox", "--deno-sandbox"] {
        let err = Args::parse([flag].into_iter().map(str::to_string)).unwrap_err();
        assert!(err.to_string().contains("unsupported option"));
    }
}

#[test]
fn serious_engineer_config_sets_prompt_and_budget() {
    let host_config = P0HostConfig {
        linked_folders: Vec::new(),
        approvals: Default::default(),
        artifact_root: None,
        proc_run_timeout_ms: tm_host::default_proc_run_timeout_ms(),
        proc_isolation: Default::default(),
        self_evolution: Default::default(),
        egress: Default::default(),
    };
    let linked = host_config.linked_folders().unwrap();
    let cfg = build_agent_config(
        &Args::default(),
        Protocol::NativeTool,
        &host_config,
        &linked,
    );
    assert_eq!(cfg.max_turns, 8);
    assert_eq!(cfg.cell_budget.output_bytes, 50_000);
    assert!(cfg.system_prompt.contains("SOUL.md"));
    assert!(cfg.system_prompt.contains("Tempest Miku"));
    assert!(
        cfg.system_prompt
            .contains("Serious Engineer Operating Notes")
    );
    assert!(cfg.system_prompt.contains("# tm Language Fluency"));
    assert!(cfg.system_prompt.contains("fun value -> expr"));
    assert!(cfg.system_prompt.contains("proc.run(cmd, args)"));
    assert!(
        cfg.system_prompt
            .contains("Treat the turn budget as an execution budget")
    );
    assert!(cfg.system_prompt.contains("Prefer `@fs.patch` patch hunks"));
    assert!(cfg.system_prompt.contains("expectedLines"));
    assert!(
        cfg.system_prompt
            .contains("Multiple `execute` calls returned in one model turn")
    );
    assert!(cfg.system_prompt.contains("not CPU-heavy gates"));
    assert!(cfg.system_prompt.contains("final four turns"));
    assert!(
        cfg.system_prompt
            .contains("Active mode: Serious Engineer. It is already selected")
    );
    assert!(cfg.system_prompt.contains("Do not call modes.suggest"));

    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".to_string(),
        path: PathBuf::from("."),
        mode: FsMode::Rw,
        commands: vec!["git".to_string()],
        safe_args: vec![vec!["git".to_string()]],
    }])
    .unwrap();
    let linked_prompt = serious_engineer_prompt(&host_config, &linked);
    assert!(linked_prompt.contains("@fs.read {path: \"repo:src/file.ts\", selector: \"120-220\"}"));
    assert!(linked_prompt.contains("never map full-file fs.read over many hits"));
    assert!(linked_prompt.contains("`fs.remove` deletes an entire file"));
    assert!(linked_prompt.contains("confirm nonzero collection"));
    assert!(linked_prompt.contains("git diff"));

    let grants = serious_engineer_grants();
    assert!(grants.permits("fs.read"));
    assert!(grants.permits("fs.patch"));
    assert!(grants.permits("fs.move"));
    assert!(grants.permits("fs.remove"));
    assert!(grants.permits("fs.grep"));
    assert!(grants.permits("proc.run"));
    assert!(grants.permits("resources.read:linked"));
    assert!(!grants.permits("agents.spawn"));
}

#[tokio::test]
async fn multi_folder_default_session_id_stays_fail_closed() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    std::fs::write(first.path().join("secret.txt"), "private\n").unwrap();
    let host_config = P0HostConfig {
        linked_folders: vec![
            LinkedFolderConfig {
                name: "first".into(),
                path: first.path().to_path_buf(),
                mode: FsMode::Rw,
                commands: Vec::new(),
                safe_args: Vec::new(),
            },
            LinkedFolderConfig {
                name: "second".into(),
                path: second.path().to_path_buf(),
                mode: FsMode::Rw,
                commands: Vec::new(),
                safe_args: Vec::new(),
            },
        ],
        approvals: Default::default(),
        artifact_root: Some(artifacts.path().to_path_buf()),
        proc_run_timeout_ms: tm_host::default_proc_run_timeout_ms(),
        proc_isolation: Default::default(),
        self_evolution: Default::default(),
        egress: Default::default(),
    };
    let linked = host_config.linked_folders().unwrap();
    let sandbox = build_sandbox(
        &Args {
            session_id: Some("default".into()),
            ..Args::default()
        },
        &host_config,
        &tm_mcp::McpRuntimeConfig::default(),
        linked,
    )
    .await
    .unwrap();
    let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

    let output = session
        .eval(
            "@fs.read {path: \"first:secret.txt\"}",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("non-project session scope cli:unscoped")),
        "{output:?}"
    );
}
