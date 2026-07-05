use std::{env, path::PathBuf};

use anyhow::{Result, bail};
use tm_e2e::{
    LiveSpeaker, MikuClient, RecordOptions, ScriptedSpeaker, WorkflowOptions, run_record_api,
    run_record_live_api, run_record_suite, run_record_ui, run_workflow, write_workflow_record,
};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse()?;

    match args {
        Args::Help => {
            print_help();
            Ok(())
        }
        Args::Legacy(args) => run_legacy(args).await,
        Args::Record(args) => run_record(args).await,
    }
}

async fn run_legacy(args: CliArgs) -> Result<()> {
    let require_artifact = env::var("TM_E2E_REQUIRE_ARTIFACT").ok().as_deref() == Some("1");
    let client = MikuClient::from_env()?;
    let options = WorkflowOptions { require_artifact };
    let report = match args.mode.as_str() {
        "scripted" => {
            let speaker = args.scripted_speaker();
            run_workflow(&client, &speaker, options).await?
        }
        "live" => {
            if args.has_message_overrides() {
                bail!("custom scripted messages are supported with scripted mode, not live mode");
            }
            let speaker = LiveSpeaker::from_env()?;
            run_workflow(&client, &speaker, options).await?
        }
        other => bail!("unsupported tm-e2e mode {other}; expected scripted or live"),
    };

    let record_path = record_path(&args.mode, args.record_json);
    write_workflow_record(&record_path, &args.mode, &report)?;

    println!("tm-e2e workflow passed");
    println!("session: {}", report.session_id);
    println!("memory: {}", report.memory_record_uri);
    if let Some(uri) = &report.artifact_uri {
        println!("artifact: {uri}");
    }
    println!("rounds: {}", report.rounds.len());
    println!("record: {}", record_path.display());
    println!("promoted: {}", report.promoted_count);
    Ok(())
}

async fn run_record(args: RecordCliArgs) -> Result<()> {
    let options = RecordOptions {
        output_dir: args.output_dir,
        headed: args.headed,
        skip_flutter_build: args.skip_flutter_build,
    };
    let manifest = match args.mode.as_str() {
        "suite" => run_record_suite(options).await?,
        "api" => run_record_api(options).await?,
        "ui" => run_record_ui(options).await?,
        "live-api" => run_record_live_api(options).await?,
        other => {
            bail!("unsupported tm-e2e record mode {other}; expected suite, api, ui, or live-api")
        }
    };
    println!("tm-e2e record {} passed", args.mode);
    println!("evidence: {}", manifest.run_dir);
    println!("manifest: {}/manifest.json", manifest.run_dir);
    println!("report: {}/report.md", manifest.run_dir);
    println!("index: {}/index.html", manifest.run_dir);
    Ok(())
}

#[derive(Debug)]
enum Args {
    Legacy(CliArgs),
    Record(RecordCliArgs),
    Help,
}

impl Args {
    fn parse() -> Result<Self> {
        let raw = env::args().skip(1).collect::<Vec<_>>();
        if raw
            .iter()
            .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
        {
            return Ok(Self::Help);
        }
        if raw.first().map(String::as_str) == Some("record") {
            return Ok(Self::Record(RecordCliArgs::parse(&raw[1..])?));
        }
        Ok(Self::Legacy(CliArgs::parse_from(raw)?))
    }
}

#[derive(Debug)]
struct CliArgs {
    mode: String,
    record_json: Option<PathBuf>,
    personal_message: Option<String>,
    coding_message: Option<String>,
}

impl CliArgs {
    fn parse_from(raw: Vec<String>) -> Result<Self> {
        let mut mode = None;
        let mut record_json = None;
        let mut personal_message = None;
        let mut coding_message = None;
        let mut args = raw.into_iter();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "help" | "--help" | "-h" => {
                    bail!("help is handled before legacy argument parsing");
                }
                "--record-json" => {
                    let Some(path) = args.next() else {
                        bail!("--record-json requires a path");
                    };
                    record_json = Some(PathBuf::from(path));
                }
                value if value.starts_with("--record-json=") => {
                    let path = value.trim_start_matches("--record-json=");
                    if path.is_empty() {
                        bail!("--record-json requires a non-empty path");
                    }
                    record_json = Some(PathBuf::from(path));
                }
                "--personal-message" | "--personal-prompt" => {
                    let Some(message) = args.next() else {
                        bail!("{arg} requires a message");
                    };
                    personal_message = Some(message);
                }
                value
                    if value.starts_with("--personal-message=")
                        || value.starts_with("--personal-prompt=") =>
                {
                    let Some((_, message)) = value.split_once('=') else {
                        bail!("{value} requires a message");
                    };
                    if message.trim().is_empty() {
                        bail!("{value} requires a non-empty message");
                    }
                    personal_message = Some(message.to_string());
                }
                "--coding-message" | "--coding-prompt" => {
                    let Some(message) = args.next() else {
                        bail!("{arg} requires a message");
                    };
                    coding_message = Some(message);
                }
                value
                    if value.starts_with("--coding-message=")
                        || value.starts_with("--coding-prompt=") =>
                {
                    let Some((_, message)) = value.split_once('=') else {
                        bail!("{value} requires a message");
                    };
                    if message.trim().is_empty() {
                        bail!("{value} requires a non-empty message");
                    }
                    coding_message = Some(message.to_string());
                }
                value if value.starts_with('-') => {
                    bail!("unsupported tm-e2e flag {value}");
                }
                value => {
                    if mode.replace(value.to_string()).is_some() {
                        bail!("multiple tm-e2e modes provided");
                    }
                }
            }
        }

        Ok(Self {
            mode: mode.unwrap_or_else(|| "scripted".to_string()),
            record_json,
            personal_message,
            coding_message,
        })
    }

    fn scripted_speaker(&self) -> ScriptedSpeaker {
        ScriptedSpeaker::new(self.personal_message.clone(), self.coding_message.clone())
    }

    fn has_message_overrides(&self) -> bool {
        self.personal_message.is_some() || self.coding_message.is_some()
    }
}

#[derive(Debug)]
struct RecordCliArgs {
    mode: String,
    output_dir: Option<PathBuf>,
    headed: bool,
    skip_flutter_build: bool,
}

impl RecordCliArgs {
    fn parse(raw: &[String]) -> Result<Self> {
        let mut mode = None;
        let mut output_dir = None;
        let mut headed = false;
        let mut skip_flutter_build = false;
        let mut args = raw.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--output-dir" | "--run-dir" => {
                    let Some(path) = args.next() else {
                        bail!("{arg} requires a path");
                    };
                    output_dir = Some(PathBuf::from(path));
                }
                value if value.starts_with("--output-dir=") || value.starts_with("--run-dir=") => {
                    let Some((_, path)) = value.split_once('=') else {
                        bail!("{value} requires a path");
                    };
                    output_dir = Some(PathBuf::from(path));
                }
                "--headed" => headed = true,
                "--skip-flutter-build" => skip_flutter_build = true,
                value if value.starts_with('-') => bail!("unsupported tm-e2e record flag {value}"),
                value => {
                    if mode.replace(value.to_string()).is_some() {
                        bail!("multiple tm-e2e record modes provided");
                    }
                }
            }
        }
        Ok(Self {
            mode: mode.unwrap_or_else(|| "suite".to_string()),
            output_dir,
            headed,
            skip_flutter_build,
        })
    }
}

fn record_path(mode: &str, cli_path: Option<PathBuf>) -> PathBuf {
    cli_path
        .or_else(|| env::var_os("TM_E2E_RECORD_PATH").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("target/tm-e2e").join(format!("{mode}-latest.json")))
}

fn print_help() {
    println!(
        "tm-e2e — drive TempestMiku through the public session API\n\n\
         Usage:\n  \
           cargo run -p tm-e2e -- scripted [--personal-message text] [--coding-message text] [--record-json path]\n  \
           TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... cargo run -p tm-e2e -- live [--record-json path]\n\n\
           cargo run -p tm-e2e -- record suite [--output-dir path] [--headed] [--skip-flutter-build]\n  \
           cargo run -p tm-e2e -- record api [--output-dir path]\n  \
           cargo run -p tm-e2e -- record ui [--output-dir path] [--headed]\n  \
           TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... cargo run -p tm-e2e -- record live-api [--output-dir path]\n\n\
         Environment:\n  \
           TM_MIKU_BASE_URL          server URL, default http://127.0.0.1:8787\n  \
           TM_MIKU_BEARER_TOKEN      optional bearer token for tm-server auth\n  \
           TM_MIKU_E2E_TIMEOUT_MS    SSE wait timeout, default 15000\n  \
           TM_E2E_REQUIRE_ARTIFACT   set 1 to require an artifact event/resource\n  \
           TM_E2E_RECORD_PATH        JSON transcript path, default target/tm-e2e/<mode>-latest.json\n  \
           TM_E2E_SPEAKER_MODEL      live-mode speaker model, default OPENAI_MODEL\n  \
           TM_E2E_SKIP_FLUTTER_BUILD set 1 to reuse clients/miku_flutter/build/web for record ui/suite"
    );
}
