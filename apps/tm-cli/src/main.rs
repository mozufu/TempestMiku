//! TempestMiku CLI.
//!
//! Wires the streaming OpenAI client to the stub sandbox and runs the agent loop, rendering
//! the model's tokens to stdout the instant they stream in. Cell telemetry goes to stderr so
//! piping stdout yields just the answer.

use std::io::{Read, Write};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tm_core::{Agent, AgentConfig, EventSink, Protocol};
use tm_llm::OpenAiClient;
use tm_sandbox::StubSandbox;

#[tokio::main]
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

    let prompt = match args.prompt {
        Some(p) => p,
        None => read_stdin().context("reading prompt from stdin")?,
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        print_usage();
        bail!("no prompt provided");
    }

    let llm = OpenAiClient::from_env().context("building OpenAI client")?;

    let model = args
        .model
        .or_else(|| std::env::var("OPENAI_MODEL").ok())
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "gpt-4o-mini".to_string());

    let protocol = if args.fenced || env_is_fenced() {
        Protocol::FencedBlock
    } else {
        Protocol::NativeTool
    };

    let cfg = AgentConfig {
        model,
        max_turns: args.max_turns.unwrap_or(12),
        protocol,
        ..AgentConfig::default()
    };

    let agent = Agent::new(Arc::new(llm), Arc::new(StubSandbox), cfg);
    let sink = StdoutSink;

    agent.run(prompt, &sink).await.context("agent run")?;
    Ok(())
}

/// Streams assistant tokens to stdout live; cell telemetry to stderr (dimmed).
struct StdoutSink;

impl EventSink for StdoutSink {
    fn on_text(&self, delta: &str) {
        print!("{delta}");
        let _ = std::io::stdout().flush();
    }

    fn on_tool_call(&self, name: &str) {
        eprintln!("\x1b[2m· tool call: {name}\x1b[0m");
    }

    fn on_cell_start(&self, code: &str) {
        eprintln!("\x1b[2m· executing cell ({} bytes)\x1b[0m", code.len());
    }

    fn on_cell_result(&self, shaped: &str) {
        eprintln!("\x1b[2m· result → model:\x1b[0m");
        for line in shaped.lines() {
            eprintln!("\x1b[2m  {line}\x1b[0m");
        }
    }

    fn on_final(&self, _text: &str) {
        println!(); // terminate the streamed answer line
    }
}

#[derive(Debug, Default)]
struct Args {
    prompt: Option<String>,
    model: Option<String>,
    max_turns: Option<usize>,
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

fn print_usage() {
    eprintln!(
        "tm — TempestMiku CLI (M0)\n\n\
         USAGE:\n  \
         tm [OPTIONS] <prompt...>\n  \
         echo <prompt> | tm\n\n\
         OPTIONS:\n  \
         --model <name>     model id (or env OPENAI_MODEL)\n  \
         --max-turns <n>    max agent turns (default 12)\n  \
         --fenced           use the fenced-block protocol (or env TM_PROTOCOL=fenced)\n  \
         -h, --help         show this help\n\n\
         ENV:\n  \
         OPENAI_BASE_URL    default https://api.openai.com/v1\n  \
         OPENAI_API_KEY     bearer token\n  \
         OPENAI_MODEL       model id\n"
    );
}
