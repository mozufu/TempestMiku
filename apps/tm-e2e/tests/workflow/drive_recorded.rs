use super::server::start_drive_smoke_server;
use super::*;

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
    assert_eq!(scenario.details["scriptedLlmCalls"], json!(7));
    assert_ne!(
        scenario.details["continuity"]["sessionId"],
        scenario.details["continuity"]["sourceSessionId"]
    );
    assert_eq!(
        scenario.details["continuity"]["modelRequestIncludedRecall"],
        json!(true)
    );
    for event_type in ["text", "final"] {
        assert!(
            scenario.details["continuity"]["replayedEventTypes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind == event_type)
        );
    }
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
