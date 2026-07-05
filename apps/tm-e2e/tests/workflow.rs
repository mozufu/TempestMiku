use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{json, to_value};
use tm_e2e::{
    E2eConfig, MikuClient, ScriptedSpeaker, WORKFLOW_RECORD_SCHEMA_VERSION, WorkflowOptions,
    run_actor_smoke, run_workflow, write_workflow_record,
};
use tm_server::{
    AppState, ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, AuthConfig,
    CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult, EchoChatRunner, InMemoryStore,
    ModeId, ServerError, StoreEvent, StoreMemoryProvider, app,
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
    assert_eq!(report.rounds[0].mode, "personal_assistant");
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
    assert_eq!(session.mode, "personal_assistant");

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
    assert_eq!(report.artifact_uri, "artifact://0");
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

    server.abort();
}

async fn start_server(
    auth: AuthConfig,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let state = AppState::new(
        store,
        memory,
        chat,
        tm_server::PersonaConfig::default(),
        auth,
    )
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

async fn start_actor_smoke_server() -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let artifact_root = temp.path().to_path_buf();
    let store = Arc::new(InMemoryStore::default());
    let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
    let chat = Arc::new(EchoChatRunner);
    let mut state = AppState::new(
        store,
        memory,
        chat,
        tm_server::PersonaConfig::default(),
        AuthConfig::NoAuth,
    )
    .with_artifact_root(artifact_root.clone());
    let broker = Arc::clone(&state.approval_broker);
    state = state.with_coding_backend(Arc::new(ActorSmokeBackend {
        root: artifact_root,
        broker,
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
        let actor_id = "Worker0";
        sink.emit(
            "actor_spawned",
            json!({
                "actor_id": actor_id,
                "role": "worker",
                "task": "actor smoke",
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
                "actor_id": actor_id,
                "summary": "child smoke complete",
                "artifact_uri": "artifact://0",
                "history_uri": "history://Worker0",
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
