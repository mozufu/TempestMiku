use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{json, to_value};
use tm_e2e::{
    E2eConfig, MikuClient, ScriptedSpeaker, WORKFLOW_RECORD_SCHEMA_VERSION, WorkflowOptions,
    run_workflow, write_workflow_record,
};
use tm_server::{
    AppState, AuthConfig, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult,
    EchoChatRunner, InMemoryStore, ModeId, ServerError, StoreEvent, StoreMemoryProvider, app,
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

struct ArtifactBackend {
    root: PathBuf,
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
