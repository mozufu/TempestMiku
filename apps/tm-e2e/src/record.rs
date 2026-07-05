use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json, to_value};
use tm_server::{
    AppState, ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, AuthConfig,
    CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult, EchoChatRunner, InMemoryStore,
    ModeId, ServerError, StoreEvent, StoreMemoryProvider, app,
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
    ensure!(
        env::var("TM_LLM_E2E_LIVE").ok().as_deref() == Some("1"),
        "record live-api is gated by TM_LLM_E2E_LIVE=1"
    );
    run_recorded("live-api", options, true, false, false, true).await
}

async fn run_recorded(
    label: &str,
    options: RecordOptions,
    include_public_api: bool,
    include_actor_api: bool,
    include_ui: bool,
    live_api: bool,
) -> Result<EvidenceManifest> {
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
        Ok::<Value, anyhow::Error>(json!({
            "sessionId": report.session_id,
            "actorId": report.actor_id,
            "approvalId": report.approval_id,
            "artifactUri": report.artifact_uri,
            "replayedEventTypes": report.replayed_event_types
        }))
    }
    .await;
    record_scenario_result(recorder, "api-actor", started_at, &result);
    result.map(|_| ())
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
        let state = AppState::new(
            store,
            memory,
            chat,
            tm_server::ModesConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_approval_broker(Arc::clone(&broker))
        .with_artifact_root(artifact_root.clone())
        .with_coding_backend(Arc::new(RecordingBackend {
            root: artifact_root.clone(),
            broker,
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
        let actor_id = "Worker0";
        sink.emit(
            "actor_spawned",
            json!({
                "actor_id": actor_id,
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
                        "actorId": actor_id,
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
                "actor_id": actor_id,
                "summary": "child smoke complete",
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker0",
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
