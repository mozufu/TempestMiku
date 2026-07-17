use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use serde_json::{json, to_value};
use tm_agents::{ActorBudget, ActorId, ActorRecord, ActorStatus, FailureReason, MailboxRegistry};
use tm_core::{
    AgentConfig, CellBudget, ChatRequest, Error as CoreError, LlmClient, Message,
    Result as CoreResult, StreamEvent,
};
use tm_e2e::{
    E2eConfig, E2eEvent, EVIDENCE_SCHEMA_VERSION, MikuClient, RecordOptions, ScriptedSpeaker,
    WORKFLOW_RECORD_SCHEMA_VERSION, WorkflowOptions, run_actor_smoke, run_drive_smoke,
    run_record_api, run_record_native_coding, run_workflow, write_workflow_record,
};
use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};
use tm_lang::{TmSandbox, TmSandboxOptions};
use tm_server::{
    AppState, ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, AuthConfig,
    ChatActorExecutor, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult,
    EchoChatRunner, HttpApprovalPolicy, InMemoryStore, ModeId, NativeApprovalMode, NativeTmBackend,
    RosterCodingEventSink, ServerError, StoreEvent, StoreMemoryProvider, app,
};

#[tokio::test]
async fn scripted_workflow_drives_miku_public_api() {
    let (base_url, server, temp) = start_server(AuthConfig::NoAuth).await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(10),
    })
    .unwrap();

    let speaker = ScriptedSpeaker::default();
    let report = run_workflow(
        &client,
        &speaker,
        WorkflowOptions {
            require_artifact: true,
        },
    )
    .await
    .unwrap();

    assert!(report.personal_final.contains("Miku heard:"));
    assert!(report.coding_final.contains("Decision:"));
    assert!(report.memory_record_uri.starts_with("memory://profile/"));
    assert_eq!(report.artifact_uri.as_deref(), Some("artifact://0"));
    assert!(report.promoted_count >= 4);
    assert_eq!(report.rounds.len(), 2);
    assert_eq!(report.rounds[0].index, 1);
    assert_eq!(report.rounds[0].mode, "general");
    assert!(
        report.rounds[0]
            .event_types
            .iter()
            .any(|kind| kind == "text")
    );
    assert_eq!(report.rounds[1].index, 2);
    assert_eq!(report.rounds[1].mode, "serious_engineer");
    assert!(
        report.rounds[1]
            .resource_uris
            .iter()
            .any(|uri| uri == "artifact://0")
    );

    let record_path = temp.path().join("workflow-record.json");
    write_workflow_record(&record_path, "scripted", &report).unwrap();
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(
        record["schemaVersion"],
        json!(WORKFLOW_RECORD_SCHEMA_VERSION)
    );
    assert_eq!(record["mode"], json!("scripted"));
    assert_eq!(record["sessionId"], json!(report.session_id));
    assert_eq!(record["rounds"].as_array().unwrap().len(), 2);

    server.abort();
}

#[tokio::test]
async fn bearer_token_is_sent_when_configured() {
    let (base_url, server, _temp) =
        start_server(AuthConfig::BearerToken("secret".to_string())).await;
    let unauthenticated = MikuClient::new(E2eConfig {
        base_url: base_url.clone(),
        bearer_token: None,
        timeout: Duration::from_secs(5),
    })
    .unwrap();
    assert!(unauthenticated.create_session(None).await.is_err());

    let authenticated = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: Some("secret".to_string()),
        timeout: Duration::from_secs(5),
    })
    .unwrap();
    let session = authenticated.create_session(None).await.unwrap();
    assert_eq!(session.mode, "general");

    server.abort();
}

#[tokio::test]
async fn actor_smoke_covers_progress_approval_resource_and_replay() {
    let (base_url, server, _temp) = start_actor_smoke_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(10),
    })
    .unwrap();

    let report = run_actor_smoke(&client).await.unwrap();

    assert_eq!(report.actor_id, "Worker0");
    assert_eq!(report.agent_uri, "agent://Worker0");
    assert_eq!(report.artifact_uri, "artifact://0");
    assert_eq!(report.history_uri, "history://Worker0");
    assert_eq!(report.cancelled_actor_id, "CancelledWorker");
    assert_eq!(report.cancelled_agent_uri, "agent://CancelledWorker");
    assert!(report.approval_id.len() > 8);
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_spawned".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"approval".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"approval_resolved".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_completed".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_resources_linked".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"actor_cancelled".to_string())
    );

    server.abort();
}

#[tokio::test]
async fn drive_smoke_public_api_covers_p5_drop_approval_resource_and_replay() {
    let (base_url, server, _temp) = start_drive_smoke_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(15),
    })
    .unwrap();

    let report = run_drive_smoke(&client).await.unwrap();

    assert_eq!(report.filed_uri, "drive://inbox/approval-drop.md");
    assert!(report.approval_id.len() > 8);
    assert!(
        report
            .replayed_event_types
            .contains(&"approval".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"approval_resolved".to_string())
    );
    assert!(
        report
            .replayed_event_types
            .contains(&"drive_put".to_string())
    );

    server.abort();
}

#[tokio::test]
async fn native_tm_actor_coordination_public_api_covers_p3_plus_route() {
    let (base_url, server, _temp) = start_native_actor_coordination_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(20),
    })
    .unwrap();

    let session = client.create_session(Some("handoff")).await.unwrap();
    let (_, mode_event) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await
        .unwrap();
    let replay_anchor = mode_event.id;

    let send_client = client.clone();
    let send_session_id = session.id.clone();
    let send = tokio::spawn(async move {
        send_client
            .send_message(
                &send_session_id,
                "exercise native P3+ actor coordination route",
            )
            .await
    });
    let live_events = client
        .read_until_final(&session.id, replay_anchor)
        .await
        .unwrap();
    send.await.unwrap().unwrap();

    let final_text = live_events
        .iter()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.data["text"].as_str())
        .unwrap_or_default();
    assert!(final_text.contains(NATIVE_P3_FINAL_TEXT));

    let (first_link_batch, first_link) = client
        .wait_for_event(&session.id, replay_anchor, |event| {
            event.event_type == "actor_resources_linked"
        })
        .await
        .unwrap();
    let (second_link_batch, second_link) = client
        .wait_for_event(&session.id, first_link.id, |event| {
            event.event_type == "actor_resources_linked"
        })
        .await
        .unwrap();
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
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_spawned")
            .count()
            >= 2,
        "expected two actor_spawned events, saw {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_message")
            .count()
            >= 4,
        "expected broadcast and child reply actor_message events, saw {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_completed")
            .count()
            >= 2,
        "expected two actor_completed events, saw {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .filter(|kind| **kind == "actor_resources_linked")
            .count()
            >= 2,
        "expected two actor_resources_linked events, saw {event_types:?}"
    );
    assert!(
        event_types.contains(&"final"),
        "expected final event in replayed/native route events"
    );
    let artifact_uris = [
        first_link.data["artifact_uri"].as_str().unwrap(),
        second_link.data["artifact_uri"].as_str().unwrap(),
    ];
    assert_ne!(
        artifact_uris[0], artifact_uris[1],
        "child actor artifact links should be distinct"
    );

    for linked in [first_link, second_link] {
        assert_native_child_resources(&client, &session.id, &linked).await;
    }

    server.abort();
}

#[tokio::test]
async fn native_tm_http_sse_e2e_approves_and_replays_structured_trace() {
    let (base_url, server, temp) = start_native_tm_server().await;
    let client = MikuClient::new(E2eConfig {
        base_url,
        bearer_token: None,
        timeout: Duration::from_secs(10),
    })
    .unwrap();
    let session = client
        .create_session(Some("serious_engineer"))
        .await
        .unwrap();
    client
        .set_session_scope(&session.id, "project:repo")
        .await
        .unwrap();
    let (_, mode) = client
        .wait_for_event(&session.id, Some(0), |event| event.event_type == "mode")
        .await
        .unwrap();
    let anchor = mode.id;

    let send_client = client.clone();
    let send_session = session.id.clone();
    let send = tokio::spawn(async move {
        send_client
            .send_message(&send_session, "approve the tm e2e fixture removal")
            .await
    });
    let (before_approval, approval) = client
        .wait_for_event(&session.id, anchor, |event| event.event_type == "approval")
        .await
        .unwrap();
    assert!(before_approval.iter().any(|event| {
        event.event_type == "effect_suspended" && event.data["nodeId"].as_str().is_some()
    }));
    let approval_id = approval.data["approvalId"].as_str().unwrap();
    client
        .resolve_approval(&session.id, approval_id, "approve")
        .await
        .unwrap();
    let live_tail = client
        .read_until_final(&session.id, approval.id)
        .await
        .unwrap();
    send.await.unwrap().unwrap();
    assert!(!temp.path().join("repo/remove-me.txt").exists());

    let replay = client.read_until_final(&session.id, anchor).await.unwrap();
    for expected in [
        "scope_start",
        "effect_start",
        "effect_suspended",
        "approval",
        "approval_resolved",
        "effect_resumed",
        "effect_result",
        "scope_result",
        "display",
        "binding_committed",
        "cell_result",
        "final",
    ] {
        assert!(
            replay.iter().any(|event| event.event_type == expected),
            "missing {expected}: {:?}",
            replay
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>()
        );
    }
    let remove_start = replay
        .iter()
        .find(|event| event.event_type == "effect_start" && event.data["capability"] == "fs.remove")
        .unwrap();
    assert_eq!(remove_start.data["argsPreview"], "[redacted]");
    assert!(live_tail.iter().any(|event| event.event_type == "final"));
    server.abort();
}

#[tokio::test]
async fn recorded_api_suite_writes_evidence_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let run_dir = temp.path().join("record-api");

    let manifest = run_record_api(RecordOptions {
        output_dir: Some(run_dir.clone()),
        headed: false,
        skip_flutter_build: true,
    })
    .await
    .unwrap();

    assert!(manifest.ok);
    assert_eq!(manifest.schema_version, EVIDENCE_SCHEMA_VERSION);
    assert!(run_dir.join("manifest.json").exists());
    assert!(run_dir.join("events.ndjson").exists());
    assert!(run_dir.join("http.ndjson").exists());
    assert!(run_dir.join("transcript.md").exists());
    assert!(run_dir.join("report.md").exists());
    assert!(run_dir.join("index.html").exists());
    assert!(
        manifest
            .scenarios
            .iter()
            .any(|scenario| scenario.name == "api-public" && scenario.ok)
    );
    assert!(
        manifest
            .scenarios
            .iter()
            .any(|scenario| scenario.name == "api-actor" && scenario.ok)
    );
    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri.starts_with("memory://profile/"))
    );
    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri == "artifact://0")
    );
    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri == "history://Worker0")
    );
    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri == "agent://Worker0")
    );
    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri == "agent://CancelledWorker")
    );

    let transcript = std::fs::read_to_string(run_dir.join("transcript.md")).unwrap();
    assert!(transcript.contains("## api-actor — PASS"));
    assert!(transcript.contains("history://Worker0"));
    assert!(transcript.contains("agent://CancelledWorker"));
    assert!(transcript.contains("actor_resources_linked"));
    assert!(transcript.contains("actor_cancelled"));

    let history = resolved_resource(&run_dir, &manifest, "history://Worker0");
    assert!(
        history["content"]
            .as_str()
            .unwrap()
            .contains("child smoke transcript")
    );
    assert!(
        history["content"]
            .as_str()
            .unwrap()
            .contains("[cell_result] artifact://0")
    );
    let cancelled = resolved_resource(&run_dir, &manifest, "agent://CancelledWorker");
    let cancelled_record: serde_json::Value =
        serde_json::from_str(cancelled["content"].as_str().unwrap()).unwrap();
    assert_eq!(cancelled_record["status"], json!("terminated"));
    assert_eq!(cancelled_record["cancelled"], json!(true));
    assert_eq!(
        cancelled_record["failure_reason"]["kind"],
        json!("cancelled")
    );
}

#[tokio::test]
async fn recorded_native_coding_proves_public_api_edit_test_approval_and_replay() {
    let temp = tempfile::tempdir().unwrap();
    let run_dir = temp.path().join("record-native-coding");

    let manifest = run_record_native_coding(RecordOptions {
        output_dir: Some(run_dir.clone()),
        headed: false,
        skip_flutter_build: true,
    })
    .await
    .unwrap();

    assert!(manifest.ok);
    assert_eq!(manifest.schema_version, EVIDENCE_SCHEMA_VERSION);
    assert_eq!(
        manifest.server.as_ref().unwrap().coding_backend,
        "native-tm-scripted-offline"
    );
    let scenario = manifest
        .scenarios
        .iter()
        .find(|scenario| scenario.name == "native-coding")
        .unwrap();
    assert!(scenario.ok);
    assert_eq!(scenario.details["approved"]["status"], json!("approved"));
    assert_eq!(scenario.details["denied"]["status"], json!("denied"));
    assert_eq!(scenario.details["timedOut"]["status"], json!("timed_out"));
    assert_eq!(scenario.details["scriptedLlmCalls"], json!(6));
    for path in ["approved", "denied", "timedOut"] {
        assert!(
            scenario.details[path]["turnId"]
                .as_str()
                .is_some_and(|turn_id| !turn_id.is_empty())
        );
        let replay = scenario.details[path]["replayedEventTypes"]
            .as_array()
            .unwrap();
        for event_type in [
            "tool_call",
            "cell_start",
            "approval",
            "approval_resolved",
            "cell_result",
            "final",
        ] {
            assert!(replay.iter().any(|kind| kind == event_type));
        }
    }

    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri == "artifact://0")
    );
    assert!(
        manifest
            .resources
            .iter()
            .any(|resource| resource.uri == "linked://repo/src/lib.rs")
    );
    let spill = resolved_resource(&run_dir, &manifest, "artifact://0");
    assert!(
        spill["content"]
            .as_str()
            .unwrap()
            .contains("native coding evidence output")
    );
    let linked = resolved_resource(&run_dir, &manifest, "linked://repo/src/lib.rs");
    assert!(linked["content"].as_str().unwrap().contains("    2\n"));
    assert_eq!(
        std::fs::read_to_string(run_dir.join("native-coding-repo/guard.txt")).unwrap(),
        "unchanged\n"
    );
    assert_eq!(
        std::fs::read_to_string(run_dir.join("native-coding-repo/timeout.txt")).unwrap(),
        "unchanged\n"
    );

    let events = std::fs::read_to_string(run_dir.join("events.ndjson")).unwrap();
    assert!(events.contains("\"turnId\":"));
    assert!(events.contains("\"status\":\"approved\""));
    assert!(events.contains("\"status\":\"denied\""));
    assert!(events.contains("\"status\":\"timed_out\""));
    let http = std::fs::read_to_string(run_dir.join("http.ndjson")).unwrap();
    assert_eq!(http.matches("/approvals/").count(), 2);
}

fn resolved_resource(
    run_dir: &std::path::Path,
    manifest: &tm_e2e::EvidenceManifest,
    uri: &str,
) -> serde_json::Value {
    let resource = manifest
        .resources
        .iter()
        .find(|resource| resource.uri == uri)
        .unwrap_or_else(|| panic!("missing recorded resource {uri}"));
    serde_json::from_slice(&std::fs::read(run_dir.join(&resource.resolve_path)).unwrap()).unwrap()
}

const NATIVE_P3_BROADCAST_TEXT: &str = "native P3 plus broadcast token";
const NATIVE_P3_FINAL_TEXT: &str = "native P3 plus coordination complete";

async fn assert_native_child_resources(client: &MikuClient, session_id: &str, linked: &E2eEvent) {
    let actor_id = linked.data["actor_id"]
        .as_str()
        .expect("actor_resources_linked actor_id");
    let history_uri = linked.data["history_uri"]
        .as_str()
        .expect("actor_resources_linked history_uri");
    let artifact_uri = linked.data["artifact_uri"]
        .as_str()
        .expect("actor_resources_linked artifact_uri");
    assert_eq!(history_uri, format!("history://{actor_id}"));
    assert!(artifact_uri.starts_with("artifact://"));

    let artifact = client
        .resolve_resource(session_id, artifact_uri)
        .await
        .unwrap();
    let artifact_content = artifact["content"].as_str().unwrap();
    assert!(
        artifact_content.contains(NATIVE_P3_BROADCAST_TEXT),
        "artifact {artifact_uri} did not contain broadcast token: {artifact_content}"
    );

    let history = client
        .resolve_resource(session_id, history_uri)
        .await
        .unwrap();
    let history_content = history["content"].as_str().unwrap();
    assert!(history_content.contains("agents.wait"));
    assert!(history_content.contains("[cell_result]"));
    assert!(history_content.contains(NATIVE_P3_BROADCAST_TEXT));

    let agent_uri = format!("agent://{actor_id}");
    let agent = client
        .resolve_resource(session_id, &agent_uri)
        .await
        .unwrap();
    let record: serde_json::Value =
        serde_json::from_str(agent["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["status"], json!("terminated"));
    assert_eq!(record["cancelled"], json!(false));
    assert_eq!(record["artifact_uri"], json!(artifact_uri));
    assert_eq!(record["history_uri"], json!(history_uri));
}

async fn start_server(
    auth: AuthConfig,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(store, memory, chat, tm_server::ModesConfig::default(), auth)
        .with_auto_turn_dispatcher(true)
        .with_linked_folders(test_linked_project(temp.path()))
        .with_artifact_root(artifact_root.clone())
        .with_coding_backend(Arc::new(ArtifactBackend {
            root: artifact_root,
        }));
    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

async fn start_native_actor_coordination_server()
-> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let parent_code = native_parent_coordination_code();
    let child_code = native_child_coordination_code();
    let llm = Arc::new(ScriptedLlm::new(vec![
        execute_script("parent_call", &parent_code),
        execute_script("child_call_0", &child_code),
        execute_script("child_call_1", &child_code),
        text_script(NATIVE_P3_FINAL_TEXT),
        text_script(NATIVE_P3_FINAL_TEXT),
        text_script(NATIVE_P3_FINAL_TEXT),
        text_script(NATIVE_P3_FINAL_TEXT),
    ]));
    let cfg = AgentConfig {
        model: "fake".to_string(),
        max_turns: 6,
        cell_budget: CellBudget {
            wall_ms: 240_000,
            output_bytes: 50_000,
        },
        ..AgentConfig::default()
    };

    let roster = Arc::new(MailboxRegistry::new());
    let mut sandbox_options = TmSandboxOptions {
        artifact_root: artifact_root.clone(),
        approval_timeout: Duration::from_secs(5),
        ..TmSandboxOptions::default()
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
                opts.grants =
                    tm_lang::core_tm_grants().allow_many(grants.names().map(str::to_string));
                let sink: Arc<dyn CodingEventSink> = Arc::new(RosterCodingEventSink::new(
                    session_id,
                    Arc::clone(&executor_approval_roster),
                ));
                opts.approval_policy = Arc::new(
                    HttpApprovalPolicy::new(Arc::clone(&executor_broker), session_id, sink)
                        .with_actor_id(actor_id.map(str::to_string)),
                );
                Arc::new(TmSandbox::new(opts)) as Arc<dyn tm_core::Sandbox>
            },
            Some(artifact_root.clone()),
            executor_roster,
        ));
    roster.set_executor(executor);

    let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
    let backend = NativeTmBackend::new(
        llm_for_backend,
        cfg,
        sandbox_options,
        NativeApprovalMode::Manual,
        broker,
    );
    state = state.with_coding_backend(Arc::new(backend));
    state.wire_lifecycle_sink();

    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

async fn start_native_tm_server() -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let linked_root = temp.path().join("repo");
    std::fs::create_dir_all(&linked_root).unwrap();
    std::fs::write(linked_root.join("remove-me.txt"), "delete-me\n").unwrap();
    let linked = LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "repo".into(),
        path: linked_root,
        mode: FsMode::Rw,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .unwrap();
    let code = r#"
let hits = @code.search {pattern: "delete-me", paths: ["repo:remove-me.txt"], regex: false};
hits |> par map (fun hit -> @fs.remove {path: hit.path, tag: hit.tag}) |> display {kind: "table"}
"#;
    let llm = Arc::new(ScriptedLlm::new(vec![
        execute_script("tm_e2e_edit", code),
        text_script("tm e2e complete"),
    ]));
    let llm_client: Arc<dyn LlmClient> = llm;
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
    let backend = NativeTmBackend::new(
        llm_client,
        AgentConfig {
            model: "fake".into(),
            max_turns: 3,
            cell_budget: CellBudget {
                wall_ms: 30_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        },
        TmSandboxOptions {
            artifact_root,
            linked_folders: Some(linked),
            approval_timeout: Duration::from_secs(5),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        Arc::clone(&state.approval_broker),
    );
    state = state.with_coding_backend(Arc::new(backend));
    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

async fn start_drive_smoke_server() -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().join("artifacts");
    let drive_store = tm_drive::InMemoryDriveStore::new(
        tm_artifacts::ArtifactStore::open(temp.path(), "drive").unwrap(),
    );
    let llm = Arc::new(ScriptedLlm::new(vec![
        execute_script("drive_smoke_call", &drive_smoke_code()),
        text_script("drive smoke complete"),
    ]));
    let cfg = AgentConfig {
        model: "fake".to_string(),
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
    .with_linked_folders(test_linked_project(temp.path()))
    .with_artifact_root(artifact_root.clone())
    .with_drive_store(drive_store.clone());
    let broker = Arc::clone(&state.approval_broker);
    let backend = NativeTmBackend::new(
        llm,
        cfg,
        TmSandboxOptions {
            artifact_root,
            drive_store: Some(Arc::new(drive_store)),
            approval_timeout: Duration::from_secs(5),
            ..TmSandboxOptions::default()
        },
        NativeApprovalMode::Manual,
        broker,
    );
    state = state.with_coding_backend(Arc::new(backend));

    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

fn drive_smoke_code() -> String {
    r##"
let filed = @drive.put {content: "# Approval Drop\nManual approval gates drive writes.\nResearch smoke citation body.", options: {
  auto: true,
  suggestedPath: "inbox/approval-drop.md",
  project: "tempestmiku",
  docKind: "note",
  sourceUri: "drop://browser/approval-drop.md",
  eventSeq: 101
}};
let hits = @drive.search {query: "approval", project: "tempestmiku", returnSnippets: true};
let researchResult = @research.drive {
  query: "approval",
  project: "tempestmiku",
  maxDocs: 1,
  maxSnippets: 1,
  maxWorkers: 0,
  maxBytesPerDoc: 200,
  maxDigestBytes: 120
};
let citation = match researchResult.citations {
  | first :: _ -> first
  | [] -> {sourceKind: "missing"}
};
{
  filedUri: filed.uri,
  sourceUri: filed.entry.sourceUri,
  searchHits: length hits,
  researchCitations: length researchResult.citations,
  sourceKind: citation.sourceKind,
  answerHasDriveUri: contains "drive://" researchResult.answer
} |> display {kind: "json"}
"##
    .to_string()
}

fn test_linked_project(root: &std::path::Path) -> LinkedFolders {
    LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: root.to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .expect("active TempestMiku test project")
}

fn native_parent_coordination_code() -> String {
    format!(
        r#"
let alpha = @agents.spawn {{role: "worker", task: "Wait for the parent broadcast, write a short artifact, and send Root a report."}};
let beta = @agents.spawn {{role: "worker", task: "Wait for the parent broadcast, write a short artifact, and send Root a report."}};
let readyA = @agents.wait {{from: alpha, timeoutMs: 15000}};
let readyB = @agents.wait {{from: beta, timeoutMs: 15000}};
let receipts = @agents.broadcast {{text: "{broadcast}"}};
let first = @agents.wait {{from: alpha, timeoutMs: 15000}};
let second = @agents.wait {{from: beta, timeoutMs: 15000}};
let roster = @agents.list {{}};
{{
  receipts,
  ready: [readyA.text, readyB.text],
  reports: [first.text, second.text],
  roster
}} |> display {{kind: "json"}}
"#,
        broadcast = NATIVE_P3_BROADCAST_TEXT
    )
}

fn native_child_coordination_code() -> String {
    r#"
let ready = @agents.send {to: "Root", text: "child ready for native broadcast"};
let msg = @agents.wait {from: "Root", timeoutMs: 15000};
let text = msg.text;
let artifact = @artifacts.put {data: "native child saw: #{text}", title: "native p3 child"};
let report = @agents.send {to: "Root", text: "child report #{artifact.uri}: #{text}"};
{text, artifact: artifact.uri} |> display {kind: "json"}
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

fn text_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::Text(text.to_string()),
        StreamEvent::Finish {
            reason: Some("stop".to_string()),
        },
    ]
}

struct ScriptedLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ScriptedLlm {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
        self.requests
            .lock()
            .map_err(|_| CoreError::Llm("scripted request lock poisoned".to_string()))?
            .push(req.messages.clone());
        let events = self
            .scripts
            .lock()
            .map_err(|_| CoreError::Llm("scripted LLM lock poisoned".to_string()))?
            .pop_front()
            .ok_or_else(|| CoreError::Llm("scripted LLM exhausted".to_string()))?;
        Ok(Box::pin(stream::iter(
            events.into_iter().map(Ok::<StreamEvent, CoreError>),
        )))
    }
}

async fn start_actor_smoke_server() -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let roster = Arc::new(MailboxRegistry::new());
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
    state = state.with_coding_backend(Arc::new(ActorSmokeBackend {
        root: artifact_root,
        broker,
        roster,
    }));
    let router = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), server, temp)
}

struct ArtifactBackend {
    root: PathBuf,
}

struct ActorSmokeBackend {
    root: PathBuf,
    broker: Arc<ApprovalBroker>,
    roster: Arc<MailboxRegistry>,
}

#[async_trait]
impl CodingBackend for ArtifactBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        assert_eq!(turn.mode, ModeId::from("serious_engineer"));
        let store = tm_artifacts::ArtifactStore::open(&self.root, turn.session_id.to_string())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let artifact = store
            .put_text(
                "tm-e2e transcript artifact\n",
                Some("tm-e2e transcript".to_string()),
                "text/plain",
            )
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sink.emit("text", json!({ "delta": "coding through backend" }))
            .await?;
        sink.emit(
            "artifact",
            json!({
                "backend": "tm-e2e-test",
                "artifact": artifact,
            }),
        )
        .await?;
        let final_text = "E2E coding backend complete. Open loop: keep the hatch covered. Decision: keep the hatch HTTP-only. artifact://0".to_string();
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

#[async_trait]
impl CodingBackend for ActorSmokeBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        assert_eq!(turn.mode, ModeId::from("handoff"));
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
                "task": "actor smoke",
            }),
        )
        .await?;
        let approval = self
            .broker
            .request_permission_detailed_for_backend(
                turn.session_id,
                "native-tm",
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
                Duration::from_secs(5),
                Arc::clone(&sink),
            )
            .await?;
        assert_eq!(approval.status, ApprovalStatus::Approved);

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
                "backend": "tm-e2e-actor-smoke",
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
        let final_text = "Actor smoke complete with artifact://0".to_string();
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
