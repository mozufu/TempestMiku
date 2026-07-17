use super::server::start_server;
use super::*;

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
