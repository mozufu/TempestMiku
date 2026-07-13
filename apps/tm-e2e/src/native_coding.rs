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
use futures::stream::{self, BoxStream};
use serde_json::{Value, json};
use tm_core::{
    AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Result as CoreResult,
    StreamEvent,
};
use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};
use tm_sandbox::DenoSandboxOptions;
use tm_server::{
    AppState, AuthConfig, EchoChatRunner, InMemoryStore, NativeApprovalMode, NativeDenoBackend,
    NativeDenoBackendOptions, StoreMemoryProvider, app,
};

use crate::{
    E2eConfig, E2eEvent, EvidenceManifest, EvidenceRecorder, MikuClient, RecordOptions,
    ServerEvidence, default_run_dir, timestamp,
};

const LINK_ALIAS: &str = "repo";
const APPROVED_FINAL: &str = "native coding approved path complete";
const DENIED_FINAL: &str = "native coding denied path complete";
const TIMEOUT_FINAL: &str = "native coding timeout path complete";
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn run_record_native_coding(options: RecordOptions) -> Result<EvidenceManifest> {
    let root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| default_run_dir("native-coding"));
    let command = env::args().collect::<Vec<_>>().join(" ");
    let recorder = EvidenceRecorder::create(&root, command)?;
    let result = run_record_native_coding_inner(&recorder).await;
    let manifest = recorder.finish(result.is_ok())?;
    if let Err(err) = result {
        bail!(
            "tm-e2e record native-coding failed: {err}; evidence: {}",
            manifest.run_dir
        );
    }
    Ok(manifest)
}

async fn run_record_native_coding_inner(recorder: &EvidenceRecorder) -> Result<()> {
    recorder.append_transcript(format!(
        "- Run `native-coding` started at `{}`.",
        timestamp()
    ));
    let started_at = timestamp();
    let result = run_native_coding_scenario(recorder).await;
    recorder.record_scenario(crate::scenario_result(
        "native-coding",
        started_at,
        result
            .as_ref()
            .cloned()
            .unwrap_or_else(|_| json!({ "status": "failed" })),
        &result
            .as_ref()
            .map(|_| ())
            .map_err(|err| anyhow::anyhow!(err.to_string())),
    ));
    result.map(|_| ())
}

async fn run_native_coding_scenario(recorder: &EvidenceRecorder) -> Result<Value> {
    let mut server = NativeCodingRecordingServer::start(&recorder.root()).await?;
    recorder.set_server(ServerEvidence {
        base_url: server.base_url.clone(),
        artifact_root: server.artifact_root.display().to_string(),
        store: "in-memory".to_string(),
        coding_backend: "native-deno-scripted-offline".to_string(),
    });
    let client = MikuClient::new(E2eConfig {
        base_url: server.base_url.clone(),
        bearer_token: None,
        timeout: Duration::from_secs(30),
    })?
    .with_recorder(recorder.clone());

    let result = run_native_coding_paths(recorder, &client, &server).await;
    server.shutdown().await;
    result
}

async fn run_native_coding_paths(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    server: &NativeCodingRecordingServer,
) -> Result<Value> {
    let (session_id, mode_event_id) = start_native_session(client).await?;
    let approved = run_approved_path(
        recorder,
        client,
        &server.linked_root,
        &session_id,
        mode_event_id,
    )
    .await?;
    let denied = run_denied_path(
        client,
        &server.linked_root,
        &session_id,
        Some(approved.last_event_id),
    )
    .await?;
    let timed_out = run_timeout_path(
        client,
        &server.linked_root,
        &session_id,
        Some(denied.last_event_id),
    )
    .await?;
    let llm_calls = server.llm_calls.load(Ordering::SeqCst);
    ensure!(
        llm_calls == 6,
        "three native turns should use one execute and one final LLM call each; saw {llm_calls}"
    );

    let source_path = server.linked_root.join("src/lib.rs");
    let guard_path = server.linked_root.join("guard.txt");
    let timeout_path = server.linked_root.join("timeout.txt");
    recorder.add_artifact("native-coding-source", &source_path)?;
    recorder.add_artifact("native-coding-deny-guard", &guard_path)?;
    recorder.add_artifact("native-coding-timeout-guard", &timeout_path)?;
    recorder.append_transcript(format!(
        "- Native coding approved `{}`, denied `{}`, timed out `{}`; scripted LLM calls `{llm_calls}`.",
        approved.session_id, denied.session_id, timed_out.session_id
    ));
    recorder.append_transcript(format!(
        "- Native coding spill resource: `{}`; edited linked resource: `{}`.",
        approved.artifact_uri, approved.linked_uri
    ));

    Ok(json!({
        "approved": approved.to_json(),
        "denied": denied.to_json(),
        "timedOut": timed_out.to_json(),
        "scriptedLlmCalls": llm_calls,
    }))
}

struct PathReport {
    session_id: String,
    turn_id: String,
    approval_id: String,
    status: String,
    replayed_event_types: Vec<String>,
    artifact_uri: Option<String>,
    linked_uri: Option<String>,
    last_event_id: i64,
}

impl PathReport {
    fn to_json(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "turnId": self.turn_id,
            "approvalId": self.approval_id,
            "status": self.status,
            "replayedEventTypes": self.replayed_event_types,
            "artifactUri": self.artifact_uri,
            "linkedUri": self.linked_uri,
        })
    }
}

async fn run_approved_path(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    linked_root: &Path,
    session_id: &str,
    replay_anchor: Option<i64>,
) -> Result<ApprovedPathReport> {
    let turn_id = client
        .send_message(
            session_id,
            "Patch the fixture, run its targeted test, and preserve large process output.",
        )
        .await?;
    let (_, approval) = client
        .wait_for_event(session_id, replay_anchor, is_native_approval)
        .await?;
    ensure!(
        approval.data["action"] == json!("proc.run cargo test answer_is_two -- --nocapture"),
        "approved path should gate the targeted cargo test, got {}",
        approval.data
    );
    let approval_id = approval_id(&approval)?;
    client
        .resolve_approval(session_id, &approval_id, "approve")
        .await?;
    let replayed = client.read_until_final(session_id, replay_anchor).await?;
    ensure_turn_provenance(&replayed, &turn_id)?;
    ensure_resolution(&replayed, "approved")?;
    let display = cell_display(&replayed)?;
    ensure!(
        display["editChanged"] == json!(true)
            && display["exitCode"] == json!(0)
            && display["truncated"] == json!(true),
        "approved native result did not prove edit, test, and spill: {display}"
    );
    let artifact_uri = display["artifactUri"]
        .as_str()
        .context("approved native result did not include artifactUri")?
        .to_string();
    ensure!(
        artifact_uri.starts_with("artifact://"),
        "proc.run spill should return an artifact URI, got {artifact_uri}"
    );
    let linked_uri = format!("linked://{LINK_ALIAS}/src/lib.rs");
    capture_resource(recorder, client, session_id, &artifact_uri).await?;
    capture_resource(recorder, client, session_id, &linked_uri).await?;
    let source = fs::read_to_string(linked_root.join("src/lib.rs"))
        .context("reading patched native coding fixture")?;
    ensure!(
        source.contains("    2\n"),
        "code.edit did not patch the linked fixture: {source}"
    );
    let final_text = final_text(&replayed)?;
    ensure!(
        final_text.contains(APPROVED_FINAL) && !final_text.contains('喵'),
        "approved final did not preserve the serious voice cap: {final_text}"
    );
    let replayed_event_types = event_types(&replayed);
    ensure_native_replay(&replayed_event_types)?;
    let last_event_id = last_event_id(&replayed)?;
    Ok(ApprovedPathReport {
        session_id: session_id.to_string(),
        turn_id,
        approval_id,
        replayed_event_types,
        artifact_uri,
        linked_uri,
        last_event_id,
    })
}

async fn run_denied_path(
    client: &MikuClient,
    linked_root: &Path,
    session_id: &str,
    replay_anchor: Option<i64>,
) -> Result<PathReport> {
    let turn_id = client
        .send_message(session_id, "Try the guarded overwrite and handle denial.")
        .await?;
    let (_, approval) = client
        .wait_for_event(session_id, replay_anchor, is_native_approval)
        .await?;
    ensure!(
        approval.data["action"] == json!("fs.write overwrite repo:guard.txt"),
        "denied path should gate the guard overwrite, got {}",
        approval.data
    );
    let approval_id = approval_id(&approval)?;
    client
        .resolve_approval(session_id, &approval_id, "deny")
        .await?;
    let replayed = client.read_until_final(session_id, replay_anchor).await?;
    ensure_turn_provenance(&replayed, &turn_id)?;
    ensure_resolution(&replayed, "denied")?;
    let display = cell_display(&replayed)?;
    ensure!(
        display["name"] == json!("ApprovalDeniedError"),
        "denied overwrite should return ApprovalDeniedError: {display}"
    );
    ensure!(
        fs::read_to_string(linked_root.join("guard.txt"))? == "unchanged\n",
        "denied overwrite mutated guard.txt"
    );
    ensure!(final_text(&replayed)?.contains(DENIED_FINAL));
    let replayed_event_types = event_types(&replayed);
    ensure_native_replay(&replayed_event_types)?;
    let last_event_id = last_event_id(&replayed)?;
    Ok(PathReport {
        session_id: session_id.to_string(),
        turn_id,
        approval_id,
        status: "denied".to_string(),
        replayed_event_types,
        artifact_uri: None,
        linked_uri: None,
        last_event_id,
    })
}

async fn run_timeout_path(
    client: &MikuClient,
    linked_root: &Path,
    session_id: &str,
    replay_anchor: Option<i64>,
) -> Result<PathReport> {
    let turn_id = client
        .send_message(
            session_id,
            "Try the guarded overwrite and let approval time out.",
        )
        .await?;
    let (_, approval) = client
        .wait_for_event(session_id, replay_anchor, is_native_approval)
        .await?;
    ensure!(
        approval.data["action"] == json!("fs.write overwrite repo:timeout.txt"),
        "timeout path should gate the timeout guard overwrite, got {}",
        approval.data
    );
    let approval_id = approval_id(&approval)?;
    let replayed = client.read_until_final(session_id, replay_anchor).await?;
    ensure_turn_provenance(&replayed, &turn_id)?;
    ensure_resolution(&replayed, "timed_out")?;
    let display = cell_display(&replayed)?;
    ensure!(
        display["name"] == json!("ApprovalTimeoutError"),
        "timed-out overwrite should return ApprovalTimeoutError: {display}"
    );
    ensure!(
        fs::read_to_string(linked_root.join("timeout.txt"))? == "unchanged\n",
        "timed-out overwrite mutated timeout.txt"
    );
    ensure!(final_text(&replayed)?.contains(TIMEOUT_FINAL));
    let replayed_event_types = event_types(&replayed);
    ensure_native_replay(&replayed_event_types)?;
    let last_event_id = last_event_id(&replayed)?;
    Ok(PathReport {
        session_id: session_id.to_string(),
        turn_id,
        approval_id,
        status: "timed_out".to_string(),
        replayed_event_types,
        artifact_uri: None,
        linked_uri: None,
        last_event_id,
    })
}

struct ApprovedPathReport {
    session_id: String,
    turn_id: String,
    approval_id: String,
    replayed_event_types: Vec<String>,
    artifact_uri: String,
    linked_uri: String,
    last_event_id: i64,
}

impl ApprovedPathReport {
    fn to_json(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "turnId": self.turn_id,
            "approvalId": self.approval_id,
            "status": "approved",
            "replayedEventTypes": self.replayed_event_types,
            "artifactUri": self.artifact_uri,
            "linkedUri": self.linked_uri,
        })
    }
}

async fn start_native_session(client: &MikuClient) -> Result<(String, Option<i64>)> {
    let session = client
        .create_session_scoped(Some("serious_engineer"), Some("project:repo"))
        .await?;
    ensure!(
        session.mode == "serious_engineer" && session.voice_cap == "off",
        "native coding session should start in Serious Engineer with voice cap off"
    );
    let (_, mode) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await?;
    Ok((session.id, mode.id))
}

fn is_native_approval(event: &E2eEvent) -> bool {
    event.event_type == "approval" && event.data["backend"] == json!("native-deno")
}

fn approval_id(event: &E2eEvent) -> Result<String> {
    event.data["approvalId"]
        .as_str()
        .map(str::to_string)
        .context("native approval did not include approvalId")
}

fn ensure_resolution(events: &[E2eEvent], expected_status: &str) -> Result<()> {
    ensure!(
        events.iter().any(|event| {
            event.event_type == "approval_resolved"
                && event.data["backend"] == json!("native-deno")
                && event.data["status"] == json!(expected_status)
        }),
        "native replay did not include {expected_status} approval resolution: {events:#?}"
    );
    Ok(())
}

fn cell_display(events: &[E2eEvent]) -> Result<Value> {
    events
        .iter()
        .filter(|event| event.event_type == "cell_result")
        .filter_map(|event| event.data["shaped"].as_str())
        .find_map(|shaped| {
            shaped
                .lines()
                .find_map(|line| line.strip_prefix("display: "))
                .and_then(|display| serde_json::from_str(display).ok())
        })
        .context("native replay did not include a JSON display cell result")
}

fn final_text(events: &[E2eEvent]) -> Result<&str> {
    events
        .iter()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.data["text"].as_str())
        .context("native replay did not include final text")
}

fn event_types(events: &[E2eEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| event.event_type.clone())
        .collect()
}

fn last_event_id(events: &[E2eEvent]) -> Result<i64> {
    events
        .iter()
        .filter_map(|event| event.id)
        .max()
        .context("native replay did not include numeric event ids")
}

fn ensure_native_replay(event_types: &[String]) -> Result<()> {
    for expected in [
        "tool_call",
        "cell_start",
        "approval",
        "approval_resolved",
        "cell_result",
        "final",
    ] {
        ensure!(
            event_types.iter().any(|kind| kind == expected),
            "Last-Event-ID replay is missing {expected}: {event_types:?}"
        );
    }
    Ok(())
}

fn ensure_turn_provenance(events: &[E2eEvent], expected_turn_id: &str) -> Result<()> {
    ensure!(
        !events.is_empty()
            && events
                .iter()
                .all(|event| event.turn_id.as_deref() == Some(expected_turn_id)),
        "replayed events did not retain durable turn provenance {expected_turn_id}: {events:#?}"
    );
    Ok(())
}

async fn capture_resource(
    recorder: &EvidenceRecorder,
    client: &MikuClient,
    session_id: &str,
    uri: &str,
) -> Result<()> {
    let preview = client.preview_resource(session_id, uri).await?;
    let resolved = client.resolve_resource(session_id, uri).await?;
    recorder.record_resource(session_id, uri, &preview, &resolved)?;
    Ok(())
}

struct NativeCodingRecordingServer {
    base_url: String,
    artifact_root: PathBuf,
    linked_root: PathBuf,
    llm_calls: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl NativeCodingRecordingServer {
    async fn start(run_root: &Path) -> Result<Self> {
        let artifact_root = run_root.join("native-coding-artifacts");
        let linked_root = run_root.join("native-coding-repo");
        ensure!(
            !linked_root.exists(),
            "native coding evidence repo already exists at {}; choose a fresh output directory",
            linked_root.display()
        );
        create_fixture_repo(&linked_root)?;
        fs::create_dir_all(&artifact_root)
            .with_context(|| format!("creating {}", artifact_root.display()))?;

        let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: LINK_ALIAS.to_string(),
            path: linked_root.clone(),
            mode: FsMode::Rw,
            commands: vec!["cargo".to_string()],
            safe_args: Vec::new(),
        }])
        .context("configuring native coding evidence repo")?;
        let scripts = vec![
            execute_script("native_approved", approved_code()),
            text_script(APPROVED_FINAL),
            execute_script("native_denied", denied_code()),
            text_script(DENIED_FINAL),
            execute_script("native_timeout", timeout_code()),
            text_script(TIMEOUT_FINAL),
        ];
        let llm = Arc::new(OfflineScriptedLlm::new(scripts));
        let llm_calls = Arc::clone(&llm.calls);
        let llm_for_backend: Arc<dyn LlmClient> = llm;
        let cfg = AgentConfig {
            model: "offline-native-coding-script".to_string(),
            max_turns: 3,
            cell_budget: CellBudget {
                wall_ms: 240_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        };

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
        .with_linked_folders(linked.clone());
        let backend = NativeDenoBackend::new_with_options(
            llm_for_backend,
            cfg,
            DenoSandboxOptions {
                artifact_root: artifact_root.clone(),
                linked_folders: Some(linked),
                approval_timeout: APPROVAL_TIMEOUT,
                ..DenoSandboxOptions::default()
            },
            NativeApprovalMode::Manual,
            Arc::clone(&state.approval_broker),
            NativeDenoBackendOptions {
                shard_count: 1,
                ..NativeDenoBackendOptions::default()
            },
        );
        state = state.with_coding_backend(Arc::new(backend));
        state.wire_lifecycle_sink();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding native coding evidence server")?;
        let addr = listener
            .local_addr()
            .context("reading evidence server addr")?;
        let router = app(state);
        let handle = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("tm-e2e native coding server exited: {err}");
            }
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            artifact_root,
            linked_root,
            llm_calls,
            handle,
        })
    }

    async fn shutdown(&mut self) {
        self.handle.abort();
        let _ = (&mut self.handle).await;
    }
}

impl Drop for NativeCodingRecordingServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn create_fixture_repo(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("src"))
        .with_context(|| format!("creating native coding fixture at {}", root.display()))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"tm-native-coding-evidence\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )?;
    fs::write(
        root.join("src/lib.rs"),
        "pub fn answer() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn answer_is_two() {\n        assert_eq!(super::answer(), 2);\n        println!(\"native coding evidence output {}\", \"x\".repeat(2048));\n    }\n}\n",
    )?;
    fs::write(root.join("guard.txt"), "unchanged\n")?;
    fs::write(root.join("timeout.txt"), "unchanged\n")?;
    Ok(())
}

fn approved_code() -> &'static str {
    r#"
const hits = await code.search({ pattern: "    1", paths: ["repo:src/lib.rs"], regex: false });
if (hits.length !== 1) throw new Error(`expected one patch target, saw ${hits.length}`);
const hit = hits[0];
const edit = await code.edit({
  path: hit.path,
  tag: hit.tag,
  hunks: [{ op: "replace", startLine: hit.line, endLine: hit.line, lines: ["    2"] }]
});
const run = await proc.run("cargo", ["test", "answer_is_two", "--", "--nocapture"], {
  cwd: "repo:",
  outputBytes: 1
});
display({
  editChanged: edit.changed,
  exitCode: run.exitCode,
  truncated: run.truncated,
  artifactUri: run.artifact?.uri ?? null
});
"#
}

fn denied_code() -> &'static str {
    r#"
const result = await fs.write("repo:guard.txt", "mutated\n", { overwrite: true })
  .then(() => ({ name: "unexpected-success" }))
  .catch((err) => ({ name: err.name, retryable: err.retryable }));
display(result);
"#
}

fn timeout_code() -> &'static str {
    r#"
const result = await fs.write("repo:timeout.txt", "mutated\n", { overwrite: true })
  .then(() => ({ name: "unexpected-success" }))
  .catch((err) => ({ name: err.name, retryable: err.retryable }));
display(result);
"#
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

fn text_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::Text(text.to_string()),
        StreamEvent::Finish {
            reason: Some("stop".to_string()),
        },
    ]
}

struct OfflineScriptedLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    calls: Arc<AtomicUsize>,
}

impl OfflineScriptedLlm {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient for OfflineScriptedLlm {
    async fn chat_stream(
        &self,
        _request: &ChatRequest,
    ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let script = self
            .scripts
            .lock()
            .map_err(|_| CoreError::Llm("native coding script lock poisoned".to_string()))?
            .pop_front()
            .ok_or_else(|| CoreError::Llm("native coding script exhausted".to_string()))?;
        Ok(Box::pin(stream::iter(
            script.into_iter().map(Ok::<StreamEvent, CoreError>),
        )))
    }
}
