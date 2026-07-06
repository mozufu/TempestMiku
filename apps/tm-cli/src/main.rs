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
use tm_artifacts::default_root;
use tm_core::{Agent, AgentConfig, CellBudget, EventSink, Protocol, Sandbox};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, DefaultDenyApprovalPolicy, HostError, LinkedFolders,
    P0HostConfig,
};
use tm_llm::OpenAiClient;
use tm_modes::{ModeId, ModesConfig};
use tm_sandbox::{DenoSandbox, DenoSandboxOptions, StubSandbox};

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
    let sink = StdoutSink::stdio();

    agent.run(prompt, &sink).await.context("agent run")?;
    Ok(())
}

/// Streams assistant tokens to stdout live; cell telemetry to stderr (dimmed).
type StdoutSink = StreamingSink<std::io::Stdout, std::io::Stderr>;

struct StreamingSink<O, E> {
    stdout: Mutex<O>,
    stderr: Mutex<E>,
    dim: bool,
}

impl StdoutSink {
    fn stdio() -> Self {
        StreamingSink::new(std::io::stdout(), std::io::stderr(), true)
    }
}

impl<O, E> StreamingSink<O, E> {
    fn new(stdout: O, stderr: E, dim: bool) -> Self {
        Self {
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
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
}

impl<O, E> EventSink for StreamingSink<O, E>
where
    O: Write + Send,
    E: Write + Send,
{
    fn on_text(&self, delta: &str) {
        let mut stdout = self.stdout.lock();
        let _ = write!(stdout, "{delta}");
        let _ = stdout.flush();
    }

    fn on_tool_call(&self, name: &str) {
        self.write_stderr_line(format!("· tool call: {name}"));
    }

    fn on_cell_start(&self, code: &str) {
        self.write_stderr_line(format!("· executing cell ({} bytes)", code.len()));
    }

    fn on_cell_result(&self, shaped: &str) {
        self.write_stderr_line("· result → model:");
        for line in shaped.lines() {
            self.write_stderr_line(format!("  {line}"));
        }
    }

    fn on_final(&self, _text: &str) {
        let mut stdout = self.stdout.lock();
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }
}

#[derive(Debug, Default)]
struct Args {
    prompt: Option<String>,
    model: Option<String>,
    max_turns: Option<usize>,
    config: Option<PathBuf>,
    session_id: Option<String>,
    stub_sandbox: bool,
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
                "--stub-sandbox" => out.stub_sandbox = true,
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
    let mut capability_notes = String::new();
    if linked_folders.is_empty() {
        capability_notes
            .push_str("No linked folders configured; fs.*, code.*, and proc.* will fail closed.");
    } else {
        for policy in linked_folders.policies() {
            let mode = match policy.mode {
                tm_host::FsMode::Ro => "ro",
                tm_host::FsMode::Rw => "rw",
            };
            capability_notes.push_str(&format!(
                "Linked folders: {} ({mode}) at linked://{}/\n",
                policy.alias, policy.alias
            ));
        }
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
    if args.stub_sandbox {
        return Ok(Arc::new(StubSandbox));
    }
    let linked_folders = (!linked_folders.is_empty()).then_some(linked_folders);
    Ok(Arc::new(DenoSandbox::new(DenoSandboxOptions {
        artifact_root: host_config
            .artifact_root
            .clone()
            .unwrap_or_else(default_root),
        session_id: args.session_id.clone().unwrap_or_else(|| "cli".to_string()),
        linked_folders,
        approval_policy: approval_policy(host_config)?,
        approval_timeout: Duration::from_millis(host_config.approvals.timeout_ms),
        ..DenoSandboxOptions::default()
    })))
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
         --stub-sandbox     use the M0 stub sandbox for protocol debugging\n  \
         --fenced           use the fenced-block protocol (or env TM_PROTOCOL=fenced)\n  \
         -h, --help         show this help\n\n\
         ENV:\n  \
         OPENAI_BASE_URL    default https://api.openai.com/v1\n  \
         OPENAI_API_KEY     bearer token\n  \
         OPENAI_MODEL       model id\n  \
         TM_CONFIG          P0 JSON config path\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn args_parse_p0_config_session_and_stub_flags() {
        let args = Args::parse(
            [
                "--config",
                ".tempestmiku/config.json",
                "--session-id",
                "smoke",
                "--stub-sandbox",
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
        assert!(args.stub_sandbox);
        assert_eq!(args.max_turns, Some(3));
        assert_eq!(args.prompt, Some("hello".to_string()));
    }

    #[test]
    fn serious_engineer_config_sets_voice_cap_and_budget() {
        let host_config = P0HostConfig {
            linked_folders: Vec::new(),
            approvals: Default::default(),
            artifact_root: None,
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
        assert!(cfg.system_prompt.contains("proc.run(cmd, args)"));
    }
}
