use std::{
    collections::VecDeque,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use serde::Deserialize;
use serde_json::{Value, json, to_value};
use tm_agents::{ActorBudget, ActorId, ActorRecord, ActorStatus, FailureReason, MailboxRegistry};
use tm_core::{
    AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Message,
    Result as CoreResult, StreamEvent, ToolChoice,
};
use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};
use tm_llm::OpenAiClient;
use tm_sandbox::{DenoSandbox, DenoSandboxOptions};
use tm_server::{
    AppState, ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, AuthConfig,
    ChatActorExecutor, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult,
    EchoChatRunner, HttpApprovalPolicy, InMemoryStore, ModeId, NativeApprovalMode,
    NativeDenoBackend, RosterCodingEventSink, ServerError, StoreEvent, StoreMemoryProvider, app,
};
use tokio::process::Command;

use crate::{
    E2eConfig, EvidenceManifest, EvidenceRecorder, LiveSpeaker, MikuClient, ScriptedSpeaker,
    ServerEvidence, UiEvidence, WorkflowOptions, default_run_dir, run_actor_smoke, run_workflow,
    timestamp, write_workflow_record,
};

#[derive(Debug, Clone, Default)]
pub struct RecordOptions {
    pub output_dir: Option<PathBuf>,
    pub headed: bool,
    pub skip_flutter_build: bool,
}

pub async fn run_record_suite(options: RecordOptions) -> Result<EvidenceManifest> {
    run_recorded("suite", options, true, true, true, false).await
}

pub async fn run_record_api(options: RecordOptions) -> Result<EvidenceManifest> {
    run_recorded("api", options, true, true, false, false).await
}

pub async fn run_record_ui(options: RecordOptions) -> Result<EvidenceManifest> {
    run_recorded("ui", options, false, false, true, false).await
}

pub async fn run_record_live_api(options: RecordOptions) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    ensure!(
        env::var("TM_LLM_E2E_LIVE").ok().as_deref() == Some("1"),
        "record live-api is gated by TM_LLM_E2E_LIVE=1"
    );
    run_recorded("live-api", options, true, false, false, true).await
}

pub async fn run_record_native_actor(options: RecordOptions) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    ensure!(
        env::var("TM_LLM_E2E_LIVE").ok().as_deref() == Some("1"),
        "record native-actor is gated by TM_LLM_E2E_LIVE=1"
    );
    ensure_live_llm_env("record native-actor")?;

    let root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| default_run_dir("native-actor"));
    let command = env::args().collect::<Vec<_>>().join(" ");
    let recorder = EvidenceRecorder::create(&root, command)?;
    let result = run_record_native_actor_inner(&recorder).await;
    let manifest = recorder.finish(result.is_ok())?;
    if let Err(err) = result {
        bail!(
            "tm-e2e record native-actor failed: {err}; evidence: {}",
            manifest.run_dir
        );
    }
    Ok(manifest)
}

async fn run_recorded(
    label: &str,
    options: RecordOptions,
    include_public_api: bool,
    include_actor_api: bool,
    include_ui: bool,
    live_api: bool,
) -> Result<EvidenceManifest> {
    crate::load_dotenv();
    let root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| default_run_dir(label));
    let command = env::args().collect::<Vec<_>>().join(" ");
    let recorder = EvidenceRecorder::create(&root, command)?;
    let result = run_recorded_inner(
        label,
        &recorder,
        &options,
        include_public_api,
        include_actor_api,
        include_ui,
        live_api,
    )
    .await;
    let manifest = recorder.finish(result.is_ok())?;
    if let Err(err) = result {
        bail!(
            "tm-e2e record {label} failed: {err}; evidence: {}",
            manifest.run_dir
        );
    }
    Ok(manifest)
}

async fn run_recorded_inner(
    label: &str,
    recorder: &EvidenceRecorder,
    options: &RecordOptions,
    include_public_api: bool,
    include_actor_api: bool,
    include_ui: bool,
    live_api: bool,
) -> Result<()> {
    recorder.append_transcript(format!("- Run `{label}` started at `{}`.", timestamp()));

    let (client, _server) = if live_api {
        (
            MikuClient::from_env()?.with_recorder(recorder.clone()),
            None,
        )
    } else {
        let server = RecordingServer::start(&recorder.root()).await?;
        recorder.set_server(ServerEvidence {
            base_url: server.base_url.clone(),
            artifact_root: server.artifact_root.display().to_string(),
            store: "in-memory".to_string(),
            coding_backend: "tm-e2e-recording-fixture".to_string(),
        });
        let client = MikuClient::new(E2eConfig {
            base_url: server.base_url.clone(),
            bearer_token: None,
            timeout: Duration::from_secs(30),
        })?
        .with_recorder(recorder.clone());
        (client, Some(server))
    };

    if include_public_api {
        run_public_api_scenario(recorder, &client, live_api).await?;
    }
    if include_actor_api {
        run_actor_api_scenario(recorder, &client).await?;
    }
    if include_ui {
        let server_url = client.base_url().to_string();
        run_ui_scenario(recorder, &client, &server_url, options).await?;
    }

    Ok(())
}

async fn run_record_native_actor_inner(recorder: &EvidenceRecorder) -> Result<()> {
    recorder.append_transcript(format!(
        "- Run `native-actor` started at `{}`.",
        timestamp()
    ));
    let live_model = live_llm_model();
    let server = NativeActorRecordingServer::start(&recorder.root(), live_model.clone()).await?;
    recorder.set_server(ServerEvidence {
        base_url: server.base_url.clone(),
        artifact_root: server.artifact_root.display().to_string(),
        store: "in-memory".to_string(),
        coding_backend: "native-deno-scripted-execute-live-final".to_string(),
    });
    let client = MikuClient::new(E2eConfig {
        base_url: server.base_url.clone(),
        bearer_token: None,
        timeout: Duration::from_secs(45),
    })?
    .with_recorder(recorder.clone());

    run_live_llm_preflight(recorder, &live_model).await?;
    run_native_actor_coordination_scenario(recorder, &client, Arc::clone(&server.live_tail_calls))
        .await
}

fn ensure_live_llm_env(label: &str) -> Result<()> {
    let api_key_set = env::var("OPENAI_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let base_url_set = env::var("OPENAI_BASE_URL")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    ensure!(
        api_key_set || base_url_set,
        "{label} needs OPENAI_API_KEY or OPENAI_BASE_URL in the environment/.env"
    );
    Ok(())
}

fn live_llm_model() -> String {
    env::var("TM_LLM_MODEL")
        .or_else(|_| env::var("OPENAI_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string())
}

async fn run_live_llm_preflight(recorder: &EvidenceRecorder, model: &str) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let client =
            OpenAiClient::from_env().context("creating native-actor live preflight LLM")?;
        let turn = client
            .chat(&ChatRequest {
                model: model.to_string(),
                messages: vec![
                    Message::system("You are a deterministic integration-test endpoint."),
                    Message::user(format!(
                        "Return exactly this ASCII token and nothing else: {LIVE_PREFLIGHT_TOKEN}"
                    )),
                ],
                tools: Vec::new(),
                tool_choice: ToolChoice::None,
                temperature: Some(0.0),
                max_tokens: Some(32),
            })
            .await
            .context("running live LLM credential preflight")?;
        ensure!(
            turn.tool_calls.is_empty(),
            "live credential preflight unexpectedly returned tool calls"
        );
        ensure!(
            turn.text
                .to_ascii_uppercase()
                .contains(LIVE_PREFLIGHT_TOKEN),
            "live credential preflight did not return expected token {LIVE_PREFLIGHT_TOKEN}: {:?}",
            turn.text
        );
        recorder.append_transcript(format!(
            "- Live credential preflight: `{LIVE_PREFLIGHT_TOKEN}` via model `{model}`."
        ));
        Ok::<Value, anyhow::Error>(json!({
            "model": model,
            "expectedToken": LIVE_PREFLIGHT_TOKEN,
            "responsePreview": turn.text.chars().take(80).collect::<String>(),
        }))
    }
    .await;
    record_scenario_result(recorder, "live-llm-preflight", started_at, &result);
    result.map(|_| ())
}

async fn run_public_api_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    live_api: bool,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let report = if live_api {
            let speaker = LiveSpeaker::from_env()?;
            run_workflow(
                client,
                &speaker,
                WorkflowOptions {
                    require_artifact: false,
                },
            )
            .await?
        } else {
            let speaker = ScriptedSpeaker::default();
            run_workflow(
                client,
                &speaker,
                WorkflowOptions {
                    require_artifact: true,
                },
            )
            .await?
        };
        let record_path = recorder.root().join("api-public-workflow-record.json");
        write_workflow_record(
            &record_path,
            if live_api { "live-api" } else { "api-public" },
            &report,
        )?;
        recorder.add_artifact("api-public workflow record", &record_path)?;
        recorder.append_transcript(format!("- Personal final: `{}`", report.personal_final));
        recorder.append_transcript(format!("- Coding final: `{}`", report.coding_final));
        capture_resource(
            recorder,
            client,
            &report.session_id,
            &report.memory_record_uri,
        )
        .await?;
        if let Some(uri) = &report.artifact_uri {
            capture_resource(recorder, client, &report.session_id, uri).await?;
        }
        capture_resource(
            recorder,
            client,
            &report.session_id,
            "project://tempestmiku",
        )
        .await?;
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": report.session_id,
            "rounds": report.rounds.len(),
            "memoryRecordUri": report.memory_record_uri,
            "artifactUri": report.artifact_uri,
            "promotedCount": report.promoted_count
        }))
    }
    .await;
    record_scenario_result(recorder, "api-public", started_at, &result);
    result.map(|_| ())
}

async fn run_actor_api_scenario(recorder: &EvidenceRecorder, client: &MikuClient) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let report = run_actor_smoke(client).await?;
        capture_resource(recorder, client, &report.session_id, &report.artifact_uri).await?;
        capture_resource(recorder, client, &report.session_id, &report.history_uri).await?;
        capture_resource(recorder, client, &report.session_id, &report.agent_uri).await?;
        capture_resource(
            recorder,
            client,
            &report.session_id,
            &report.cancelled_agent_uri,
        )
        .await?;
        recorder.append_transcript(format!(
            "- Actor smoke resources: actor `{}`, approval `{}`, artifact `{}`, history `{}`, cancelled `{}`.",
            report.actor_id,
            report.approval_id,
            report.artifact_uri,
            report.history_uri,
            report.cancelled_agent_uri
        ));
        recorder.append_transcript(format!(
            "- Actor replay event types: `{}`",
            report.replayed_event_types.join("`, `")
        ));
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": report.session_id,
            "actorId": report.actor_id,
            "agentUri": report.agent_uri,
            "approvalId": report.approval_id,
            "artifactUri": report.artifact_uri,
            "historyUri": report.history_uri,
            "cancelledActorId": report.cancelled_actor_id,
            "cancelledAgentUri": report.cancelled_agent_uri,
            "replayedEventTypes": report.replayed_event_types
        }))
    }
    .await;
    record_scenario_result(recorder, "api-actor", started_at, &result);
    result.map(|_| ())
}

async fn run_native_actor_coordination_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    live_tail_calls: Arc<AtomicUsize>,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        let session = client.create_session(Some("handoff")).await?;
        let (_, mode_event) = client
            .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
            .await?;
        let replay_anchor = mode_event.id;

        let send_client = client.clone();
        let send_session_id = session.id.clone();
        let send = tokio::spawn(async move {
            send_client
                .send_message(
                    &send_session_id,
                    "exercise native P3+ actor coordination route with .env live credentials",
                )
                .await
        });
        let live_events = client.read_until_final(&session.id, replay_anchor).await?;
        send.await.context("joining native actor send task")??;
        let final_text = live_events
            .iter()
            .find(|event| event.event_type == "final")
            .and_then(|event| event.data["text"].as_str())
            .unwrap_or_default()
            .to_string();

        let (first_link_batch, first_link) = client
            .wait_for_event(&session.id, replay_anchor, |event| {
                event.event_type == "actor_resources_linked"
            })
            .await?;
        let (second_link_batch, second_link) = client
            .wait_for_event(&session.id, first_link.id, |event| {
                event.event_type == "actor_resources_linked"
            })
            .await?;
        let replayed = [
            live_events.clone(),
            first_link_batch,
            second_link_batch,
            vec![first_link.clone(), second_link.clone()],
        ]
        .concat();
        let event_types = replayed
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>();
        ensure_min_event_count(&event_types, "actor_spawned", 2)?;
        ensure_min_event_count(&event_types, "actor_message", 4)?;
        ensure_min_event_count(&event_types, "actor_completed", 2)?;
        ensure_min_event_count(&event_types, "actor_resources_linked", 2)?;
        ensure!(
            event_types.contains(&"final"),
            "native actor route did not replay a final event"
        );

        let mut resources = Vec::new();
        let mut artifact_uris = Vec::new();
        for linked in [&first_link, &second_link] {
            let actor_id = linked.data["actor_id"]
                .as_str()
                .context("actor_resources_linked actor_id")?
                .to_string();
            let artifact_uri = linked.data["artifact_uri"]
                .as_str()
                .context("actor_resources_linked artifact_uri")?
                .to_string();
            let history_uri = linked.data["history_uri"]
                .as_str()
                .context("actor_resources_linked history_uri")?
                .to_string();
            ensure!(
                history_uri == format!("history://{actor_id}"),
                "unexpected history uri {history_uri} for actor {actor_id}"
            );
            ensure_native_child_resource_contents(
                client,
                &session.id,
                &actor_id,
                &artifact_uri,
                &history_uri,
            )
            .await?;
            let agent_uri = format!("agent://{actor_id}");
            capture_resource(recorder, client, &session.id, &artifact_uri).await?;
            capture_resource(recorder, client, &session.id, &history_uri).await?;
            capture_resource(recorder, client, &session.id, &agent_uri).await?;
            artifact_uris.push(artifact_uri.clone());
            resources.push(json!({
                "actorId": actor_id,
                "artifactUri": artifact_uri,
                "historyUri": history_uri,
                "agentUri": agent_uri,
            }));
        }
        ensure!(
            artifact_uris.len() == 2 && artifact_uris[0] != artifact_uris[1],
            "native actor route expected distinct child artifact URIs, saw {artifact_uris:?}"
        );

        let live_tail_call_count = live_tail_calls.load(Ordering::SeqCst);
        ensure!(
            live_tail_call_count >= 3,
            "native actor route expected at least 3 live final LLM calls, saw {live_tail_call_count}"
        );
        recorder.append_transcript(format!(
            "- Native actor route final: `{}`",
            final_text.chars().take(240).collect::<String>()
        ));
        recorder.append_transcript(format!(
            "- Native actor replay event types: `{}`",
            event_types.join("`, `")
        ));
        recorder.append_transcript(format!(
            "- Native actor live final LLM calls: `{live_tail_call_count}`."
        ));
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": session.id,
            "finalText": final_text,
            "eventTypes": event_types,
            "resources": resources,
            "liveTailCalls": live_tail_call_count,
        }))
    }
    .await;
    record_scenario_result(recorder, "native-actor", started_at, &result);
    result.map(|_| ())
}

fn ensure_min_event_count(event_types: &[&str], event_type: &str, expected: usize) -> Result<()> {
    let actual = event_types
        .iter()
        .filter(|kind| **kind == event_type)
        .count();
    ensure!(
        actual >= expected,
        "expected at least {expected} `{event_type}` events, saw {actual}: {event_types:?}"
    );
    Ok(())
}

async fn ensure_native_child_resource_contents(
    client: &MikuClient,
    session_id: &str,
    actor_id: &str,
    artifact_uri: &str,
    history_uri: &str,
) -> Result<()> {
    let artifact = client.resolve_resource(session_id, artifact_uri).await?;
    let artifact_content = artifact["content"]
        .as_str()
        .context("artifact content string")?;
    ensure!(
        artifact_content.contains(NATIVE_P3_BROADCAST_TEXT),
        "artifact {artifact_uri} did not contain broadcast token: {artifact_content}"
    );

    let history = client.resolve_resource(session_id, history_uri).await?;
    let history_content = history["content"]
        .as_str()
        .context("history content string")?;
    ensure!(
        history_content.contains("agents.wait")
            && history_content.contains("[cell_result]")
            && history_content.contains(NATIVE_P3_BROADCAST_TEXT),
        "history {history_uri} did not contain expected native actor transcript markers"
    );

    let agent = client
        .resolve_resource(session_id, &format!("agent://{actor_id}"))
        .await?;
    let record: Value =
        serde_json::from_str(agent["content"].as_str().context("agent content string")?)?;
    ensure!(
        record["status"] == json!("terminated"),
        "actor not terminal"
    );
    ensure!(record["cancelled"] == json!(false), "actor was cancelled");
    ensure!(
        record["artifact_uri"] == json!(artifact_uri),
        "agent record artifact_uri mismatch"
    );
    ensure!(
        record["history_uri"] == json!(history_uri),
        "agent record history_uri mismatch"
    );
    Ok(())
}

async fn run_ui_scenario(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    base_url: &str,
    options: &RecordOptions,
) -> Result<()> {
    let started_at = timestamp();
    let result = async {
        if !options.skip_flutter_build
            && env::var("TM_E2E_SKIP_FLUTTER_BUILD").ok().as_deref() != Some("1")
        {
            run_command_capture(
                "flutter",
                &["build", "web"],
                Path::new("clients/miku_flutter"),
                &recorder.root().join("ui/flutter-build.stdout.log"),
                &recorder.root().join("ui/flutter-build.stderr.log"),
            )
            .await
            .context("building Flutter web client")?;
            recorder.add_artifact(
                "flutter build stdout",
                recorder.root().join("ui/flutter-build.stdout.log"),
            )?;
            recorder.add_artifact(
                "flutter build stderr",
                recorder.root().join("ui/flutter-build.stderr.log"),
            )?;
        }

        let ui = run_playwright(recorder, base_url, options.headed).await?;
        let session_id = read_ui_session_id(&recorder.root().join("ui/ui-result.json"))
            .ok()
            .flatten();
        recorder.record_ui(ui.clone());
        if let Some(session_id) = session_id {
            let _ = capture_resource(recorder, client, &session_id, "artifact://0").await;
            Ok::<Value, anyhow::Error>(json!({
                "sessionId": session_id,
                "ui": ui,
            }))
        } else {
            Ok::<Value, anyhow::Error>(json!({ "ui": ui }))
        }
    }
    .await;
    record_scenario_result(recorder, "ui-remote-control", started_at, &result);
    result.map(|_| ())
}

async fn capture_resource(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    session_id: &str,
    uri: &str,
) -> Result<()> {
    let preview = client
        .preview_resource(session_id, uri)
        .await
        .with_context(|| format!("previewing resource {uri}"))?;
    let resolved = client
        .resolve_resource(session_id, uri)
        .await
        .with_context(|| format!("resolving resource {uri}"))?;
    recorder.record_resource(session_id, uri, &preview, &resolved)?;
    Ok(())
}

fn record_scenario_result(
    recorder: &EvidenceRecorder,
    name: &str,
    started_at: String,
    result: &Result<Value>,
) {
    recorder.record_scenario(crate::RecordedScenario {
        name: name.to_string(),
        ok: result.is_ok(),
        started_at,
        finished_at: timestamp(),
        details: result
            .as_ref()
            .cloned()
            .unwrap_or_else(|_| json!({ "status": "failed" })),
        error: result.as_ref().err().map(|err| err.to_string()),
    });
}

async fn run_playwright(
    recorder: &EvidenceRecorder,
    base_url: &str,
    headed: bool,
) -> Result<UiEvidence> {
    let root = recorder.root();
    fs::create_dir_all(root.join("ui")).context("creating UI evidence dir")?;
    let stdout = root.join("ui/playwright.stdout.log");
    let stderr = root.join("ui/playwright.stderr.log");
    let mut args = vec![
        "exec",
        "playwright",
        "--",
        "test",
        "-c",
        "evidence.config.ts",
    ];
    if headed {
        args.push("--headed");
    }
    let mut command = Command::new("npm");
    command
        .args(&args)
        .current_dir("clients/miku_web")
        .env("TM_E2E_BASE_URL", base_url)
        .env("TM_E2E_RUN_DIR", &root);
    if headed {
        command.env("TM_E2E_HEADED", "1");
    }
    let output = command
        .output()
        .await
        .context("running Playwright evidence test through npm")?;
    fs::write(&stdout, &output.stdout).context("writing Playwright stdout")?;
    fs::write(&stderr, &output.stderr).context("writing Playwright stderr")?;

    let result_path = root.join("ui/ui-result.json");
    let playwright_json = root.join("ui/playwright-report.json");
    let mut ui = read_ui_evidence(&root).unwrap_or_default();
    ui.ok = output.status.success() && ui.ok;
    ui.result_path = existing_relative(&root, &result_path);
    ui.playwright_json_path = existing_relative(&root, &playwright_json);
    ui.stdout_path = existing_relative(&root, &stdout);
    ui.stderr_path = existing_relative(&root, &stderr);
    ui.console_path = existing_relative(&root, &root.join("ui/console.ndjson"));
    ui.network_path = existing_relative(&root, &root.join("ui/network.ndjson"));
    ui.artifacts = collect_ui_artifacts(&root)?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        ui.error = Some(if err.is_empty() {
            "Playwright evidence test failed".to_string()
        } else {
            err
        });
        recorder.record_ui(ui.clone());
        bail!("Playwright evidence test failed");
    }
    if !ui.ok {
        ui.error
            .get_or_insert_with(|| "Playwright evidence result did not report ok=true".to_string());
        recorder.record_ui(ui.clone());
        bail!("Playwright evidence result did not report ok=true");
    }
    Ok(ui)
}

async fn run_command_capture(
    program: &str,
    args: &[&str],
    cwd: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    fs::write(stdout_path, &output.stdout)
        .with_context(|| format!("writing {}", stdout_path.display()))?;
    fs::write(stderr_path, &output.stderr)
        .with_context(|| format!("writing {}", stderr_path.display()))?;
    ensure!(
        output.status.success(),
        "{program} {} failed; see {} and {}",
        args.join(" "),
        stdout_path.display(),
        stderr_path.display()
    );
    Ok(())
}

fn existing_relative(root: &Path, path: &Path) -> Option<String> {
    path.exists().then(|| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    })
}

fn collect_ui_artifacts(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(&root.join("ui"), root, &mut files)?;
    files.sort();
    Ok(files
        .into_iter()
        .filter(|path| {
            !matches!(
                path.as_str(),
                "ui/ui-result.json"
                    | "ui/playwright-report.json"
                    | "ui/console.ndjson"
                    | "ui/network.ndjson"
                    | "ui/playwright.stdout.log"
                    | "ui/playwright.stderr.log"
            ) && !path.starts_with("ui/playwright-html/trace/")
        })
        .collect())
}

fn collect_files(dir: &Path, root: &Path, out: &mut Vec<String>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, out)?;
        } else {
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiResultFile {
    ok: bool,
    session_id: Option<String>,
    screenshot_path: Option<String>,
    error: Option<String>,
}

fn read_ui_evidence(root: &Path) -> Result<UiEvidence> {
    let result_path = root.join("ui/ui-result.json");
    let result = read_ui_result(&result_path)?;
    Ok(UiEvidence {
        ok: result.ok,
        result_path: existing_relative(root, &result_path),
        screenshot_path: result
            .screenshot_path
            .as_deref()
            .map(Path::new)
            .and_then(|path| existing_relative(root, path)),
        error: result.error,
        ..UiEvidence::default()
    })
}

fn read_ui_session_id(path: &Path) -> Result<Option<String>> {
    Ok(read_ui_result(path)?.session_id)
}

fn read_ui_result(path: &Path) -> Result<UiResultFile> {
    let bytes = fs::read(path).with_context(|| format!("reading UI result {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decoding UI result {}", path.display()))
}

const LIVE_PREFLIGHT_TOKEN: &str = "TEMPEST_MIKU_E2E_OK";
const NATIVE_P3_BROADCAST_TEXT: &str = "native P3 plus broadcast token";
const NATIVE_P3_FINAL_TEXT: &str = "native P3 plus coordination complete";

struct NativeActorRecordingServer {
    base_url: String,
    artifact_root: PathBuf,
    handle: tokio::task::JoinHandle<()>,
    live_tail_calls: Arc<AtomicUsize>,
}

impl NativeActorRecordingServer {
    async fn start(run_root: &Path, live_model: String) -> Result<Self> {
        let artifact_root = run_root.join("native-actor-artifacts");
        fs::create_dir_all(&artifact_root)
            .with_context(|| format!("creating {}", artifact_root.display()))?;
        let parent_code = native_parent_coordination_code();
        let child_code = native_child_coordination_code();
        let live_tail_calls = Arc::new(AtomicUsize::new(0));
        let llm = Arc::new(ScriptedThenLiveLlm::new(
            vec![
                execute_script("parent_call", &parent_code),
                execute_script("child_call_0", &child_code),
                execute_script("child_call_1", &child_code),
            ],
            OpenAiClient::from_env().context("creating native actor live-tail LLM")?,
            live_model,
            Arc::clone(&live_tail_calls),
        ));
        let cfg = AgentConfig {
            model: "scripted-then-env-live".to_string(),
            max_turns: 6,
            cell_budget: CellBudget {
                wall_ms: 240_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        };

        let roster = Arc::new(MailboxRegistry::new());
        let mut sandbox_options = DenoSandboxOptions {
            artifact_root: artifact_root.clone(),
            approval_timeout: Duration::from_secs(5),
            ..DenoSandboxOptions::default()
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
                    opts.grants = tm_sandbox::core_sandbox_grants()
                        .allow_many(grants.names().map(str::to_string));
                    let sink: Arc<dyn CodingEventSink> = Arc::new(RosterCodingEventSink::new(
                        session_id,
                        Arc::clone(&executor_approval_roster),
                    ));
                    opts.approval_policy = Arc::new(
                        HttpApprovalPolicy::new(Arc::clone(&executor_broker), session_id, sink)
                            .with_actor_id(actor_id.map(str::to_string)),
                    );
                    Arc::new(DenoSandbox::new(opts)) as Arc<dyn tm_core::Sandbox>
                },
                Some(artifact_root.clone()),
                executor_roster,
            ));
        roster.set_executor(executor);

        let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
        let backend = NativeDenoBackend::new(
            llm_for_backend,
            cfg,
            sandbox_options,
            NativeApprovalMode::Manual,
            broker,
        );
        state = state.with_coding_backend(Arc::new(backend));
        state.wire_lifecycle_sink();

        let router = app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding tm-e2e native actor server")?;
        let addr = listener
            .local_addr()
            .context("reading native actor server addr")?;
        let handle = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("tm-e2e native actor server exited: {err}");
            }
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            artifact_root,
            handle,
            live_tail_calls,
        })
    }
}

impl Drop for NativeActorRecordingServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn native_parent_coordination_code() -> String {
    format!(
        r#"
const alpha = await agents.spawn("worker", "Wait for the parent broadcast, write a short artifact, and send Root a report.");
const beta = await agents.spawn("worker", "Wait for the parent broadcast, write a short artifact, and send Root a report.");
const readyA = await agents.wait(alpha, 15000);
const readyB = await agents.wait(beta, 15000);
const receipts = await agents.broadcast("{broadcast}");
const first = await agents.wait(alpha, 15000);
const second = await agents.wait(beta, 15000);
let roster = [];
for (let i = 0; i < 40; i++) {{
  roster = await agents.list();
  const done = [alpha.id, beta.id].every((id) =>
    roster.find((entry) => entry.id === id)?.status === "terminated"
  );
  if (done) break;
  await agents.wait(undefined, 100);
}}
display({{
  receipts,
  ready: [readyA?.text, readyB?.text],
  reports: [first?.text, second?.text],
  roster: roster.map((entry) => [entry.id, entry.status, entry.artifactUri, entry.historyUri])
}});
"#,
        broadcast = NATIVE_P3_BROADCAST_TEXT
    )
}

fn native_child_coordination_code() -> String {
    r#"
await agents.send("Root", "child ready for native broadcast");
const msg = await agents.wait("Root", 15000);
const text = msg?.text ?? "missing broadcast";
const artifact = artifacts.put(`native child saw: ${text}`, { title: "native p3 child" });
await agents.send("Root", `child report ${artifact.uri}: ${text}`);
display({ text, artifact: artifact.uri });
"#
    .to_string()
}

fn execute_script(id: &str, code: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some(id.to_string()),
            name: Some("execute".to_string()),
            arguments: Some(json!({ "code": code }).to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]
}

struct ScriptedThenLiveLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    live: OpenAiClient,
    live_model: String,
    live_tail_calls: Arc<AtomicUsize>,
}

impl ScriptedThenLiveLlm {
    fn new(
        scripts: Vec<Vec<StreamEvent>>,
        live: OpenAiClient,
        live_model: String,
        live_tail_calls: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            live,
            live_model,
            live_tail_calls,
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedThenLiveLlm {
    async fn chat_stream(
        &self,
        _req: &ChatRequest,
    ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
        let scripted = self
            .scripts
            .lock()
            .map_err(|_| CoreError::Llm("scripted native actor LLM lock poisoned".to_string()))?
            .pop_front();
        if let Some(events) = scripted {
            return Ok(Box::pin(stream::iter(
                events.into_iter().map(Ok::<StreamEvent, CoreError>),
            )));
        }

        self.live_tail_calls.fetch_add(1, Ordering::SeqCst);
        self.live
            .chat_stream(&ChatRequest {
                model: self.live_model.clone(),
                messages: vec![
                    Message::system("You are a deterministic integration-test endpoint."),
                    Message::user(format!(
                        "Return exactly this ASCII string and nothing else: {NATIVE_P3_FINAL_TEXT}"
                    )),
                ],
                tools: Vec::new(),
                tool_choice: ToolChoice::None,
                temperature: Some(0.0),
                max_tokens: Some(32),
            })
            .await
    }
}

struct RecordingServer {
    base_url: String,
    artifact_root: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

impl RecordingServer {
    async fn start(run_root: &Path) -> Result<Self> {
        let artifact_root = run_root.join("server-artifacts");
        fs::create_dir_all(&artifact_root)
            .with_context(|| format!("creating {}", artifact_root.display()))?;
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let broker = Arc::new(ApprovalBroker::default());
        let roster = Arc::new(MailboxRegistry::new());
        let linked_folders = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "tempestmiku".to_string(),
            path: run_root.to_path_buf(),
            mode: FsMode::Rw,
            commands: Vec::new(),
            safe_args: Vec::new(),
        }])
        .context("configuring recording-server project link")?;
        let state = AppState::new(
            store,
            memory,
            chat,
            tm_server::ModesConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(true)
        .with_approval_broker(Arc::clone(&broker))
        .with_artifact_root(artifact_root.clone())
        .with_linked_folders(linked_folders)
        .with_actor_roster(Arc::clone(&roster))
        .with_coding_backend(Arc::new(RecordingBackend {
            root: artifact_root.clone(),
            broker,
            roster,
        }));
        state.wire_lifecycle_sink();
        let router = app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding tm-e2e recording server")?;
        let addr = listener
            .local_addr()
            .context("reading recording server addr")?;
        let handle = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("tm-e2e recording server exited: {err}");
            }
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            artifact_root,
            handle,
        })
    }
}

impl Drop for RecordingServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct RecordingBackend {
    root: PathBuf,
    broker: Arc<ApprovalBroker>,
    roster: Arc<MailboxRegistry>,
}

#[async_trait]
impl CodingBackend for RecordingBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        if turn.mode == ModeId::from("handoff") {
            self.run_handoff_turn(turn, sink).await
        } else {
            self.run_serious_turn(turn, sink).await
        }
    }
}

impl RecordingBackend {
    async fn run_serious_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        assert_eq!(turn.mode, ModeId::from("serious_engineer"));
        let store = tm_artifacts::ArtifactStore::open(&self.root, turn.session_id.to_string())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let artifact = store
            .put_text(
                "recorded tm-e2e transcript artifact\n",
                Some("recorded tm-e2e transcript".to_string()),
                "text/plain",
            )
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sink.emit(
            "text",
            json!({ "delta": "coding through recorded backend " }),
        )
        .await?;
        sink.emit(
            "artifact",
            json!({
                "backend": "tm-e2e-recording-fixture",
                "artifact": artifact,
            }),
        )
        .await?;
        let final_text = "E2E coding backend complete. Open loop: keep recording evidence covered. Decision: keep tm-e2e as the orchestrator. artifact://0".to_string();
        sink.emit(
            "final",
            to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: None,
        })
    }

    async fn run_handoff_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        let actor_id =
            ActorId::new("Worker0").map_err(|err| ServerError::InvalidRequest(err.to_string()))?;
        let actor_id_text = actor_id.to_string();
        self.roster
            .track(actor_record(
                actor_id.clone(),
                "worker",
                ActorStatus::Running,
                false,
                None,
            ))
            .await;
        sink.emit(
            "actor_spawned",
            json!({
                "actor_id": actor_id_text,
                "role": "worker",
                "task": "recording actor smoke",
            }),
        )
        .await?;
        let approval = self
            .broker
            .request_permission_detailed_for_backend(
                turn.session_id,
                "native-deno",
                ApprovalPrompt {
                    action: "proc.run cargo clean".to_string(),
                    scope: json!({
                        "actorId": actor_id_text,
                        "action": "proc.run cargo clean",
                        "capability": "proc.run",
                    }),
                    options: vec![
                        ApprovalOption {
                            option_id: "allow".to_string(),
                            name: "Allow once".to_string(),
                            kind: "allow_once".to_string(),
                        },
                        ApprovalOption {
                            option_id: "reject".to_string(),
                            name: "Reject once".to_string(),
                            kind: "reject_once".to_string(),
                        },
                    ],
                },
                Duration::from_secs(30),
                Arc::clone(&sink),
            )
            .await?;
        if approval.status != ApprovalStatus::Approved {
            let cancelled_actor_id = ActorId::new("CancelledWorker")
                .map_err(|err| ServerError::InvalidRequest(err.to_string()))?;
            self.roster
                .track(actor_record(
                    cancelled_actor_id,
                    "watcher",
                    ActorStatus::Terminated,
                    true,
                    Some(FailureReason::ApprovalDenied),
                ))
                .await;
            let final_text = "Actor smoke approval denied".to_string();
            sink.emit(
                "final",
                to_value(StoreEvent::Final {
                    text: final_text.clone(),
                })?,
            )
            .await?;
            return Ok(CodingTurnResult {
                final_text,
                transcript_artifact: None,
            });
        }

        let store = tm_artifacts::ArtifactStore::open(&self.root, turn.session_id.to_string())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let artifact = store
            .put_text(
                "child smoke artifact opened through the resource gateway\n",
                Some("child smoke artifact".to_string()),
                "text/plain",
            )
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.roster
            .store_transcript(
                &actor_id,
                "child smoke transcript\n[cell_result] artifact://0\n".to_string(),
            )
            .await;
        self.roster
            .mark_complete_with_digest(
                &actor_id,
                "child smoke complete".to_string(),
                Some("artifact://0".to_string()),
                Some(format!("history://{actor_id_text}")),
            )
            .await;
        sink.emit(
            "artifact",
            json!({
                "backend": "tm-e2e-recording-actor",
                "artifact": artifact,
            }),
        )
        .await?;
        sink.emit(
            "actor_completed",
            json!({
                "actor_id": actor_id_text,
                "summary": "child smoke complete",
                "artifact_uri": "artifact://0",
                "history_uri": format!("history://{actor_id_text}"),
            }),
        )
        .await?;
        sink.emit(
            "actor_resources_linked",
            json!({
                "kind": "resources_linked",
                "actor_id": actor_id_text,
                "source_event_type": "actor_completed",
                "source_event_seq": null,
                "artifact_uri": "artifact://0",
                "history_uri": format!("history://{actor_id_text}"),
            }),
        )
        .await?;
        let cancelled_actor_id = ActorId::new("CancelledWorker")
            .map_err(|err| ServerError::InvalidRequest(err.to_string()))?;
        let cancelled_actor_id_text = cancelled_actor_id.to_string();
        self.roster
            .track(actor_record(
                cancelled_actor_id,
                "watcher",
                ActorStatus::Terminated,
                true,
                Some(FailureReason::Cancelled),
            ))
            .await;
        sink.emit(
            "actor_cancelled",
            json!({
                "kind": "cancelled",
                "actor_id": cancelled_actor_id_text,
                "cancelled_at": Utc::now(),
            }),
        )
        .await?;
        let final_text = "Actor Worker0 completed child resource artifact://0".to_string();
        sink.emit("text", json!({ "delta": final_text })).await?;
        sink.emit(
            "final",
            to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: None,
        })
    }
}

fn actor_record(
    id: ActorId,
    mode: &str,
    status: ActorStatus,
    cancelled: bool,
    failure_reason: Option<FailureReason>,
) -> ActorRecord {
    let now = Utc::now();
    ActorRecord {
        id,
        parent: None,
        status,
        mode: Some(mode.to_string()),
        budget: ActorBudget::default(),
        spawned_at: now,
        completed_at: (status == ActorStatus::Terminated).then_some(now),
        cancelled,
        failure_reason,
        last_summary: None,
        artifact_uri: None,
        history_uri: None,
    }
}
