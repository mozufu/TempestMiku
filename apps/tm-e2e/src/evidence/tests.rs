use super::*;
use anyhow::Result;
use serde_json::json;

#[test]
fn redacts_secret_keys_and_bearer_strings() {
    let value = json!({
        "authorization": "Bearer secret",
        "nested": {
            "apiKey": "abc",
            "safe": "hello"
        }
    });
    let redacted = redact_json(&value);
    assert_eq!(redacted["authorization"], json!("[REDACTED]"));
    assert_eq!(redacted["nested"]["apiKey"], json!("[REDACTED]"));
    assert_eq!(redacted["nested"]["safe"], json!("hello"));
}

#[test]
fn recorder_writes_current_schema_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = EvidenceRecorder::create(temp.path(), "tm-e2e test").unwrap();
    let started = timestamp();
    let result = Ok(());
    recorder.record_scenario(scenario_result(
        "unit",
        started,
        json!({"kind": "unit"}),
        &result,
    ));
    let manifest = recorder.finish(true).unwrap();
    assert_eq!(manifest.schema_version, EVIDENCE_SCHEMA_VERSION);
    assert!(temp.path().join("manifest.json").exists());
    assert!(temp.path().join("events.ndjson").exists());
    assert!(temp.path().join("http.ndjson").exists());
    assert!(temp.path().join("report.md").exists());
    assert!(temp.path().join("index.html").exists());
}

#[test]
fn recorder_preserves_partial_evidence_for_failed_runs() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = EvidenceRecorder::create(temp.path(), "tm-e2e failure test").unwrap();
    let started = timestamp();
    let result: Result<()> = Err(anyhow::anyhow!("intentional failure"));
    recorder.record_scenario(scenario_result(
        "failing-unit",
        started,
        json!({"kind": "unit"}),
        &result,
    ));
    let manifest = recorder.finish(false).unwrap();
    assert!(!manifest.ok);
    assert_eq!(manifest.scenarios[0].name, "failing-unit");
    assert!(!manifest.scenarios[0].ok);
    assert!(temp.path().join("manifest.json").exists());
    assert!(temp.path().join("report.md").exists());
}
