//! TempestMiku CLI.
//!
//! Wires the streaming OpenAI client to the P0 Serious Engineer sandbox and runs the agent loop,
//! rendering model tokens to stdout as they stream. Cell telemetry goes to stderr so piping stdout
//! yields just the answer.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::json;
use tm_core::{Agent, Error as CoreError, Protocol};
use tm_llm::OpenAiClient;

mod approval;
mod cli;
mod runtime;
mod sink;

use cli::{Args, env_is_fenced, load_host_config, print_usage, read_stdin};
use runtime::{build_agent_config, build_sandbox};
use sink::StdoutSink;

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

#[cfg(test)]
mod tests;
