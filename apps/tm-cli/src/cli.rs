use std::{io::Read, path::PathBuf};

use anyhow::{Context, Result, bail};
use tm_host::P0HostConfig;

#[derive(Debug, Default)]
pub(super) struct Args {
    pub(super) prompt: Option<String>,
    pub(super) model: Option<String>,
    pub(super) max_turns: Option<usize>,
    pub(super) config: Option<PathBuf>,
    pub(super) session_id: Option<String>,
    pub(super) event_log: Option<PathBuf>,
    pub(super) turn_budget_ok: bool,
    pub(super) fenced: bool,
    pub(super) help: bool,
}

impl Args {
    pub(super) fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
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

pub(super) fn env_is_fenced() -> bool {
    std::env::var("TM_PROTOCOL")
        .map(|v| v.eq_ignore_ascii_case("fenced"))
        .unwrap_or(false)
}

pub(super) fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s)
}

pub(super) fn load_host_config(path: Option<&PathBuf>) -> Result<P0HostConfig> {
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
            proc_isolation: Default::default(),
            self_evolution: Default::default(),
            egress: Default::default(),
        }),
    }
}

pub(super) fn print_usage() {
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
