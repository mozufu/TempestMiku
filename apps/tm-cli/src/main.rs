//! TempestMiku CLI.
//!
//! Wires the streaming OpenAI client to the P0 Serious Engineer sandbox and runs the agent loop,
//! rendering model tokens to stdout as they stream. Cell telemetry goes to stderr so piping stdout
//! yields just the answer.

use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tm_artifacts::default_root;
use tm_core::{Agent, AgentConfig, CellBudget, Error as CoreError, EventSink, Protocol, Sandbox};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, CapabilityGrants, DefaultDenyApprovalPolicy, HostError,
    LinkedFolders, P0HostConfig,
};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_llm::OpenAiClient;
use tm_modes::{ModeId, ModesConfig};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print_usage();
        return Ok(());
    }

    let prompt = match args.prompt.as_ref() {
        Some(p) => p.clone(),
        None => read_stdin().context("reading prompt from stdin")?,
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        print_usage();
        bail!("no prompt provided");
    }

    let llm = OpenAiClient::from_env().context("building OpenAI client")?;
    let protocol = if args.fenced || env_is_fenced() {
        Protocol::FencedBlock
    } else {
        Protocol::NativeTool
    };
    let host_config = load_host_config(args.config.as_ref())?;
    let linked_folders = host_config.linked_folders()?;
    let cfg = build_agent_config(&args, protocol, &host_config, &linked_folders);
    let sandbox = build_sandbox(&args, &host_config, linked_folders)?;

    let agent = Agent::new(Arc::new(llm), sandbox, cfg);
    let sink = StdoutSink::stdio(args.event_log.as_ref())?;

    let result = agent.run(prompt, &sink).await;
    if let Err(CoreError::TurnBudget(max_turns)) = &result {
        sink.write_event(json!({
            "type": "turn_budget_exhausted",
            "maxTurns": max_turns
        }))?;
        if args.turn_budget_ok {
            return Ok(());
        }
    }
    result.context("agent run")?;
    Ok(())
}

/// Streams assistant tokens to stdout live; cell telemetry to stderr (dimmed).
type StdoutSink = StreamingSink<std::io::Stdout, std::io::Stderr>;

struct StreamingSink<O, E> {
    stdout: Mutex<O>,
    stderr: Mutex<E>,
    event_log: Option<Mutex<Box<dyn Write + Send>>>,
    dim: bool,
}

impl StdoutSink {
    fn stdio(event_log: Option<&PathBuf>) -> Result<Self> {
        let mut sink = StreamingSink::new(std::io::stdout(), std::io::stderr(), true);
        if let Some(path) = event_log {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating event log parent {}", parent.display()))?;
            }
            let writer = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path)
                .with_context(|| format!("opening event log {}", path.display()))?;
            sink.event_log = Some(Mutex::new(Box::new(writer)));
        }
        Ok(sink)
    }
}

impl<O, E> StreamingSink<O, E> {
    fn new(stdout: O, stderr: E, dim: bool) -> Self {
        Self {
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
            event_log: None,
            dim,
        }
    }
}

impl<O, E> StreamingSink<O, E>
where
    O: Write + Send,
    E: Write + Send,
{
    fn write_stderr_line(&self, line: impl AsRef<str>) {
        let mut stderr = self.stderr.lock();
        if self.dim {
            let _ = writeln!(stderr, "\x1b[2m{}\x1b[0m", line.as_ref());
        } else {
            let _ = writeln!(stderr, "{}", line.as_ref());
        }
        let _ = stderr.flush();
    }

    fn write_event(&self, event: Value) -> tm_core::Result<()> {
        let Some(writer) = &self.event_log else {
            return Ok(());
        };
        let mut writer = writer.lock();
        serde_json::to_writer(&mut *writer, &event)
            .map_err(|err| CoreError::EventSink(err.to_string()))?;
        writeln!(writer).map_err(|err| CoreError::EventSink(err.to_string()))?;
        writer
            .flush()
            .map_err(|err| CoreError::EventSink(err.to_string()))
    }

    fn write_stdout_delta(&self, delta: &str) {
        let mut stdout = self.stdout.lock();
        let _ = write!(stdout, "{delta}");
        let _ = stdout.flush();
    }

    fn write_final_newline(&self) {
        let mut stdout = self.stdout.lock();
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }
}

impl<O, E> EventSink for StreamingSink<O, E>
where
    O: Write + Send,
    E: Write + Send,
{
    fn on_reasoning(&self, delta: &str) {
        let _ = self.write_event(json!({"type": "reasoning_delta", "text": delta}));
    }

    fn try_on_reasoning(&self, delta: &str) -> tm_core::Result<()> {
        self.write_event(json!({"type": "reasoning_delta", "text": delta}))
    }

    fn on_text(&self, delta: &str) {
        self.write_stdout_delta(delta);
        let _ = self.write_event(json!({"type": "text_delta", "text": delta}));
    }

    fn try_on_text(&self, delta: &str) -> tm_core::Result<()> {
        self.write_stdout_delta(delta);
        self.write_event(json!({"type": "text_delta", "text": delta}))
    }

    fn on_tool_call(&self, name: &str) {
        self.write_stderr_line(format!("· tool call: {name}"));
        let _ = self.write_event(json!({"type": "tool_call", "name": name}));
    }

    fn try_on_tool_call(&self, name: &str) -> tm_core::Result<()> {
        self.write_stderr_line(format!("· tool call: {name}"));
        self.write_event(json!({"type": "tool_call", "name": name}))
    }

    fn on_cell_start(&self, code: &str) {
        self.write_stderr_line(format!("· executing cell ({} bytes)", code.len()));
        let _ = self.write_event(json!({"type": "cell_start", "code": code}));
    }

    fn try_on_cell_start(&self, code: &str) -> tm_core::Result<()> {
        self.write_stderr_line(format!("· executing cell ({} bytes)", code.len()));
        self.write_event(json!({"type": "cell_start", "code": code}))
    }

    fn on_cell_result(&self, shaped: &str) {
        self.write_stderr_line("· result → model:");
        for line in shaped.lines() {
            self.write_stderr_line(format!("  {line}"));
        }
        let _ = self.write_event(json!({"type": "cell_result", "result": shaped}));
    }

    fn try_on_cell_result(&self, shaped: &str) -> tm_core::Result<()> {
        self.write_stderr_line("· result → model:");
        for line in shaped.lines() {
            self.write_stderr_line(format!("  {line}"));
        }
        self.write_event(json!({"type": "cell_result", "result": shaped}))
    }

    fn on_turn_end(&self) {
        let _ = self.write_event(json!({"type": "turn_end"}));
    }

    fn try_on_turn_end(&self) -> tm_core::Result<()> {
        self.write_event(json!({"type": "turn_end"}))
    }

    fn on_final(&self, text: &str) {
        self.write_final_newline();
        let _ = self.write_event(json!({"type": "final", "text": text}));
    }

    fn try_on_final(&self, text: &str) -> tm_core::Result<()> {
        self.write_final_newline();
        self.write_event(json!({"type": "final", "text": text}))
    }
}

#[derive(Debug, Default)]
struct Args {
    prompt: Option<String>,
    model: Option<String>,
    max_turns: Option<usize>,
    config: Option<PathBuf>,
    session_id: Option<String>,
    event_log: Option<PathBuf>,
    turn_budget_ok: bool,
    fenced: bool,
    help: bool,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut out = Args::default();
        let mut prompt_parts: Vec<String> = Vec::new();
        let mut it = args.peekable();

        while let Some(arg) = it.next() {
            match arg.as_str() {
                "-h" | "--help" => out.help = true,
                "--fenced" => out.fenced = true,
                "--model" => {
                    out.model = Some(it.next().context("--model needs a value")?);
                }
                "--max-turns" => {
                    let v = it.next().context("--max-turns needs a value")?;
                    out.max_turns = Some(v.parse().context("--max-turns must be a number")?);
                }
                "--config" => {
                    out.config = Some(PathBuf::from(it.next().context("--config needs a value")?));
                }
                "--session-id" => {
                    out.session_id = Some(it.next().context("--session-id needs a value")?);
                }
                "--event-log" => {
                    out.event_log = Some(PathBuf::from(
                        it.next().context("--event-log needs a value")?,
                    ));
                }
                "--turn-budget-ok" => out.turn_budget_ok = true,
                other if other.starts_with('-') => bail!("unsupported option {other}"),
                other => prompt_parts.push(other.to_string()),
            }
        }

        if !prompt_parts.is_empty() {
            out.prompt = Some(prompt_parts.join(" "));
        }
        Ok(out)
    }
}

fn env_is_fenced() -> bool {
    std::env::var("TM_PROTOCOL")
        .map(|v| v.eq_ignore_ascii_case("fenced"))
        .unwrap_or(false)
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s)
}

fn load_host_config(path: Option<&PathBuf>) -> Result<P0HostConfig> {
    let path = path
        .cloned()
        .or_else(|| std::env::var_os("TM_CONFIG").map(PathBuf::from))
        .or_else(|| {
            let default = PathBuf::from(".tempestmiku/config.json");
            default.exists().then_some(default)
        });
    match path {
        Some(path) => P0HostConfig::from_json_file(&path)
            .with_context(|| format!("loading P0 host config from {}", path.display())),
        None => Ok(P0HostConfig {
            linked_folders: Vec::new(),
            approvals: Default::default(),
            artifact_root: None,
            proc_run_timeout_ms: tm_host::default_proc_run_timeout_ms(),
            self_evolution: Default::default(),
        }),
    }
}

fn build_agent_config(
    args: &Args,
    protocol: Protocol,
    host_config: &P0HostConfig,
    linked_folders: &LinkedFolders,
) -> AgentConfig {
    let model = args
        .model
        .clone()
        .or_else(|| std::env::var("OPENAI_MODEL").ok())
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    AgentConfig {
        model,
        max_turns: args.max_turns.unwrap_or(8),
        protocol,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        system_prompt: serious_engineer_prompt(host_config, linked_folders),
        ..AgentConfig::default()
    }
}

fn serious_engineer_prompt(_host_config: &P0HostConfig, linked_folders: &LinkedFolders) -> String {
    let mut capability_notes = String::from(
        "Active mode: Serious Engineer. It is already selected and locked for this CLI run. \
Do not call modes.suggest or ask the user to switch modes; the listed fs.*, code.*, and proc.* \
grants are active now.\n",
    );
    let policies = linked_folders.policies();
    if policies.is_empty() {
        capability_notes
            .push_str("No linked folders configured; fs.*, code.*, and proc.* will fail closed.");
    } else {
        for policy in &policies {
            let mode = match policy.mode {
                tm_host::FsMode::Ro => "ro",
                tm_host::FsMode::Rw => "rw",
            };
            capability_notes.push_str(&format!(
                "Linked folders: {} ({mode}) at linked://{}/\n",
                policy.alias, policy.alias
            ));
        }
        let alias = &policies[0].alias;
        capability_notes.push_str(&format!(
            "\
Known linked-repo schemas; call these directly without tools.search/help:
- Search: @code.search {{pattern: \"needle\", paths: [\"{alias}:src\"], regex: false, contextLines: 2, limit: 20}}
- Read a bounded slice: @fs.read {{path: \"{alias}:src/file.ts\", selector: \"120-220\"}}
- Patch from a fresh search tag: @fs.patch {{path: hit.path, tag: hit.tag, hunks: [{{op: \"replace\", startLine: hit.line, endLine: hit.line, expectedLines: [hit.text], lines: [\"replacement\"]}}]}}
- Delete lines with an explicit range: @fs.patch {{path: hit.path, tag: hit.tag, hunks: [{{op: \"delete\", startLine: hit.line, endLine: hit.line, expectedLines: [hit.text]}}]}}
- Insert relative to a line: @fs.patch {{path: hit.path, tag: hit.tag, hunks: [{{op: \"insertAfter\", line: hit.line, expectedLine: hit.text, lines: [\"new line\"]}}]}}
- Create a new text file: @fs.write {{path: \"{alias}:test/new_test.ts\", data: \"test content\\n\", createParents: true}}
- Remove a file only with approval: @fs.remove {{path: hit.path, tag: hit.tag}}
- Run argv only: @proc.run {{cmd: \"git\", args: [\"status\", \"--short\"], cwd: \"{alias}:\"}}
Never pass a bare alias such as \"{alias}\" where a linked path is required. Deduplicate search-hit
paths before reading; never map full-file fs.read over many hits. Large files must use selector
ranges around relevant lines. Prefer bounded fs.read or `git grep` through proc.run; `sed` and
`grep` are not granted commands in the default CLI profile. `fs.remove` deletes an entire file and
requires approval; it is separate from patch operations. Replace/delete hunks must repeat the exact
current range in expectedLines, and relative inserts must repeat their anchor in expectedLine. If a
tag is stale or expected context mismatches, read/search again and retry with fresh evidence. After
changing tests, run the exact test file and confirm nonzero collection;
typechecking alone is not behavioral proof. Before finishing, run task-named gates plus git diff
and git status.\n"
        ));
    }
    ModesConfig::default()
        .build_system_prompt(
            &ModeId::from("serious_engineer"),
            tm_core::DEFAULT_SYSTEM_PROMPT,
            &capability_notes,
            // No live user message at CLI startup; always-on layered skills (e.g.
            // scope-guard) still compose, only keyword-triggered ones are skipped.
            "",
        )
        .system_prompt
}

fn build_sandbox(
    args: &Args,
    host_config: &P0HostConfig,
    linked_folders: LinkedFolders,
) -> Result<Arc<dyn Sandbox>> {
    // The standalone CLI is its own local authority boundary. A single configured
    // linked folder is unambiguous; multiple folders remain fail-closed until the
    // CLI grows an explicit project selector.
    let policies = linked_folders.policies();
    let session_scope = match policies.as_slice() {
        [policy] => Some(format!("project:{}", policy.alias)),
        [_, _, ..] => Some("cli:unscoped".to_string()),
        [] => None,
    };
    let linked_folders = (!linked_folders.is_empty()).then_some(linked_folders);
    let options = TmSandboxOptions {
        artifact_root: host_config
            .artifact_root
            .clone()
            .unwrap_or_else(default_root),
        session_id: args.session_id.clone().unwrap_or_else(|| "cli".to_string()),
        session_scope,
        linked_folders,
        grants: serious_engineer_grants(),
        approval_policy: approval_policy(host_config)?,
        approval_timeout: Duration::from_millis(host_config.approvals.timeout_ms),
        proc_run_timeout: Duration::from_millis(host_config.proc_run_timeout_ms),
        ..TmSandboxOptions::default()
    };
    Ok(Arc::new(TmSandbox::new(options)))
}

fn serious_engineer_grants() -> CapabilityGrants {
    let profile = ModesConfig::default()
        .load_assets()
        .profile_or_unknown(&ModeId::from("serious_engineer"));
    CapabilityGrants::default().allow_many(profile.capabilities)
}

fn approval_policy(config: &P0HostConfig) -> Result<Arc<dyn ApprovalPolicy>> {
    match config.approvals.mode.as_str() {
        "manual" => Ok(Arc::new(PromptApprovalPolicy)),
        "deny" | "" => Ok(Arc::new(DefaultDenyApprovalPolicy)),
        other => bail!("unsupported approval mode {other}"),
    }
}

#[derive(Debug)]
struct PromptApprovalPolicy;

#[async_trait]
impl ApprovalPolicy for PromptApprovalPolicy {
    async fn request(&self, action: &str, timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        let action = action.to_string();
        let thread_action = action.clone();
        let (tx, rx) = mpsc::channel();
        let timeout_ms = timeout.as_millis();
        std::thread::spawn(move || {
            let result = read_tty_approval(&thread_action, timeout_ms);
            let _ = tx.send(result);
        });
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => Err(HostError::ApprovalTimeout(action)),
        }
    }
}

fn read_tty_approval(action: &str, timeout_ms: u128) -> tm_host::Result<ApprovalDecision> {
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    let mut writer = tty
        .try_clone()
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    write!(
        writer,
        "Approval required: {action}\nType approve within {timeout_ms}ms to continue: "
    )
    .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    writer
        .flush()
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    let mut line = String::new();
    BufReader::new(tty)
        .read_line(&mut line)
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    if line.trim() == "approve" {
        Ok(ApprovalDecision::Approved)
    } else {
        Ok(ApprovalDecision::Denied)
    }
}

fn print_usage() {
    eprintln!(
        "tm — TempestMiku CLI (P0 Serious Engineer)\n\n\
         USAGE:\n  \
         tm [OPTIONS] <prompt...>\n  \
         echo <prompt> | tm\n\n\
         OPTIONS:\n  \
         --model <name>     model id (or env OPENAI_MODEL)\n  \
         --max-turns <n>    max agent turns (default 8)\n  \
         --config <path>    JSON config path (or env TM_CONFIG, else .tempestmiku/config.json)\n  \
         --session-id <id>  artifact session id (default cli)\n  \
         --event-log <path> write structured JSONL runtime events\n  \
         --turn-budget-ok   exit 0 after recording max-turn exhaustion\n  \
         --fenced           use the fenced-block protocol (or env TM_PROTOCOL=fenced)\n  \
         -h, --help         show this help\n\n\
         ENV:\n  \
         OPENAI_BASE_URL    default https://api.openai.com/v1\n  \
         OPENAI_API_KEY     bearer token\n  \
         OPENAI_MODEL       model id\n  \
         OPENAI_REASONING_EFFORT  none|minimal|low|medium|high|xhigh|max\n  \
         TM_CONFIG          P0 JSON config path\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_core::SessionConfig;
    use tm_host::{FsMode, LinkedFolderConfig};

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
    fn serious_engineer_config_sets_voice_cap_and_budget() {
        let host_config = P0HostConfig {
            linked_folders: Vec::new(),
            approvals: Default::default(),
            artifact_root: None,
            proc_run_timeout_ms: tm_host::default_proc_run_timeout_ms(),
            self_evolution: Default::default(),
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
        assert!(
            linked_prompt.contains("@fs.read {path: \"repo:src/file.ts\", selector: \"120-220\"}")
        );
        assert!(linked_prompt.contains("never map full-file fs.read over many hits"));
        assert!(linked_prompt.contains("`fs.remove` deletes an entire file"));
        assert!(linked_prompt.contains("confirm nonzero collection"));
        assert!(linked_prompt.contains("git diff"));

        let grants = serious_engineer_grants();
        assert!(grants.permits("fs.read"));
        assert!(grants.permits("fs.patch"));
        assert!(grants.permits("fs.move"));
        assert!(grants.permits("fs.remove"));
        assert!(grants.permits("code.search"));
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
            self_evolution: Default::default(),
        };
        let linked = host_config.linked_folders().unwrap();
        let sandbox = build_sandbox(
            &Args {
                session_id: Some("default".into()),
                ..Args::default()
            },
            &host_config,
            linked,
        )
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
}
