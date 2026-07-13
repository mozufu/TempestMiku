use std::{
    collections::BTreeMap,
    env, fs,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::E2eEvent;

pub const EVIDENCE_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitEvidence {
    pub revision: Option<String>,
    pub dirty: bool,
    pub status_short: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerEvidence {
    pub base_url: String,
    pub artifact_root: String,
    pub store: String,
    pub coding_backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordedScenario {
    pub name: String,
    pub ok: bool,
    pub started_at: String,
    pub finished_at: String,
    pub details: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordedEvent {
    pub timestamp: String,
    pub session_id: String,
    pub event_id: Option<i64>,
    pub turn_id: Option<String>,
    pub event_type: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordedHttpExchange {
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ok: bool,
    pub request: Value,
    pub response: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordedResource {
    pub session_id: String,
    pub uri: String,
    pub preview_path: String,
    pub resolve_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UiEvidence {
    pub ok: bool,
    pub result_path: Option<String>,
    pub playwright_json_path: Option<String>,
    pub screenshot_path: Option<String>,
    pub console_path: Option<String>,
    pub network_path: Option<String>,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub artifacts: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceManifest {
    pub schema_version: u32,
    pub ok: bool,
    pub started_at: String,
    pub finished_at: String,
    pub command: String,
    pub run_dir: String,
    pub git: GitEvidence,
    pub environment: BTreeMap<String, String>,
    pub server: Option<ServerEvidence>,
    pub scenarios: Vec<RecordedScenario>,
    pub resources: Vec<RecordedResource>,
    pub artifacts: BTreeMap<String, String>,
    pub ui: Option<UiEvidence>,
}

#[derive(Clone)]
pub struct EvidenceRecorder {
    inner: Arc<Mutex<EvidenceRecorderInner>>,
}

struct EvidenceRecorderInner {
    root: PathBuf,
    started_at: String,
    command: String,
    events_file: File,
    http_file: File,
    transcript: Vec<String>,
    scenarios: Vec<RecordedScenario>,
    resources: Vec<RecordedResource>,
    artifacts: BTreeMap<String, String>,
    server: Option<ServerEvidence>,
    ui: Option<UiEvidence>,
}

impl EvidenceRecorder {
    pub fn create(root: impl Into<PathBuf>, command: impl Into<String>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("resources"))
            .with_context(|| format!("creating evidence resources dir {}", root.display()))?;
        fs::create_dir_all(root.join("ui"))
            .with_context(|| format!("creating evidence ui dir {}", root.display()))?;
        let root = fs::canonicalize(&root)
            .with_context(|| format!("canonicalizing evidence dir {}", root.display()))?;
        let events_file = ndjson_file(root.join("events.ndjson"))?;
        let http_file = ndjson_file(root.join("http.ndjson"))?;
        let command = command.into();
        let started_at = timestamp();
        Ok(Self {
            inner: Arc::new(Mutex::new(EvidenceRecorderInner {
                root,
                started_at,
                command,
                events_file,
                http_file,
                transcript: vec!["# TempestMiku E2E Evidence Transcript".to_string()],
                scenarios: Vec::new(),
                resources: Vec::new(),
                artifacts: BTreeMap::new(),
                server: None,
                ui: None,
            })),
        })
    }

    pub fn root(&self) -> PathBuf {
        self.inner
            .lock()
            .expect("evidence recorder lock poisoned")
            .root
            .clone()
    }

    pub fn add_artifact(&self, label: impl Into<String>, path: impl AsRef<Path>) -> Result<()> {
        let mut inner = self.inner.lock().expect("evidence recorder lock poisoned");
        let relative = relative_path(&inner.root, path.as_ref());
        inner.artifacts.insert(label.into(), relative);
        Ok(())
    }

    pub fn set_server(&self, server: ServerEvidence) {
        self.inner
            .lock()
            .expect("evidence recorder lock poisoned")
            .server = Some(server);
    }

    pub fn record_ui(&self, ui: UiEvidence) {
        self.inner
            .lock()
            .expect("evidence recorder lock poisoned")
            .ui = Some(ui);
    }

    pub fn append_transcript(&self, line: impl Into<String>) {
        self.inner
            .lock()
            .expect("evidence recorder lock poisoned")
            .transcript
            .push(line.into());
    }

    pub fn record_scenario(&self, scenario: RecordedScenario) {
        let mut inner = self.inner.lock().expect("evidence recorder lock poisoned");
        inner.transcript.push(String::new());
        inner.transcript.push(format!(
            "## {} — {}",
            scenario.name,
            if scenario.ok { "PASS" } else { "FAIL" }
        ));
        if let Some(error) = &scenario.error {
            inner.transcript.push(format!("- Error: `{}`", error));
        }
        inner
            .transcript
            .push(format!("- Started: {}", scenario.started_at));
        inner
            .transcript
            .push(format!("- Finished: {}", scenario.finished_at));
        inner.scenarios.push(scenario);
    }

    pub fn record_event(&self, session_id: &str, event: &E2eEvent) -> Result<()> {
        let record = RecordedEvent {
            timestamp: timestamp(),
            session_id: session_id.to_string(),
            event_id: event.id,
            turn_id: event.turn_id.clone(),
            event_type: event.event_type.clone(),
            data: redact_json(&event.data),
        };
        let mut inner = self.inner.lock().expect("evidence recorder lock poisoned");
        write_ndjson(&mut inner.events_file, &record).context("writing recorded SSE event")
    }

    pub fn record_http(
        &self,
        method: &str,
        path: &str,
        status: u16,
        request: &Value,
        response: &Value,
    ) -> Result<()> {
        let record = RecordedHttpExchange {
            timestamp: timestamp(),
            method: method.to_string(),
            path: path.to_string(),
            status,
            ok: (200..300).contains(&status),
            request: truncate_json(&redact_json(request)),
            response: truncate_json(&redact_json(response)),
        };
        let mut inner = self.inner.lock().expect("evidence recorder lock poisoned");
        write_ndjson(&mut inner.http_file, &record).context("writing recorded HTTP exchange")
    }

    pub fn record_resource(
        &self,
        session_id: &str,
        uri: &str,
        preview: &Value,
        resolved: &Value,
    ) -> Result<RecordedResource> {
        let mut inner = self.inner.lock().expect("evidence recorder lock poisoned");
        let slug = resource_slug(session_id, uri, inner.resources.len() + 1);
        let preview_path = inner
            .root
            .join("resources")
            .join(format!("{slug}.preview.json"));
        let resolve_path = inner
            .root
            .join("resources")
            .join(format!("{slug}.resolve.json"));
        write_json_file(&preview_path, &redact_json(preview))?;
        write_json_file(&resolve_path, &redact_json(resolved))?;
        let recorded = RecordedResource {
            session_id: session_id.to_string(),
            uri: uri.to_string(),
            preview_path: relative_path(&inner.root, &preview_path),
            resolve_path: relative_path(&inner.root, &resolve_path),
        };
        inner.transcript.push(format!(
            "- Resource `{}` captured as `{}` and `{}`",
            uri, recorded.preview_path, recorded.resolve_path
        ));
        inner.resources.push(recorded.clone());
        Ok(recorded)
    }

    pub fn finish(&self, ok: bool) -> Result<EvidenceManifest> {
        let mut inner = self.inner.lock().expect("evidence recorder lock poisoned");
        inner
            .events_file
            .flush()
            .context("flushing recorded events")?;
        inner.http_file.flush().context("flushing recorded HTTP")?;

        let finished_at = timestamp();
        let manifest = EvidenceManifest {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            ok,
            started_at: inner.started_at.clone(),
            finished_at,
            command: inner.command.clone(),
            run_dir: inner.root.display().to_string(),
            git: git_evidence(),
            environment: sanitized_environment(),
            server: inner.server.clone(),
            scenarios: inner.scenarios.clone(),
            resources: inner.resources.clone(),
            artifacts: inner.artifacts.clone(),
            ui: inner.ui.clone(),
        };

        write_transcript(&inner.root, &inner.transcript)?;
        write_json_file(inner.root.join("manifest.json"), &manifest)?;
        write_report(&inner.root, &manifest)?;
        write_index(&inner.root, &manifest)?;
        validate_manifest_paths(&inner.root, &manifest)?;
        Ok(manifest)
    }
}

pub fn scenario_result(
    name: impl Into<String>,
    started_at: String,
    details: Value,
    result: &Result<()>,
) -> RecordedScenario {
    RecordedScenario {
        name: name.into(),
        ok: result.is_ok(),
        started_at,
        finished_at: timestamp(),
        details,
        error: result.as_ref().err().map(|err| err.to_string()),
    }
}

pub fn default_run_dir(label: &str) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    PathBuf::from("target")
        .join("tm-e2e")
        .join("runs")
        .join(format!("{stamp}-{label}"))
}

pub fn timestamp() -> String {
    Utc::now().to_rfc3339()
}

pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, value) in map {
                if is_secret_key(key) {
                    redacted.insert(key.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    redacted.insert(key.clone(), redact_json(value));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.iter().map(redact_json).collect()),
        Value::String(value) => Value::String(redact_string(value)),
        other => other.clone(),
    }
}

fn redact_string(value: &str) -> String {
    if value.to_ascii_lowercase().contains("bearer ") {
        return "[REDACTED]".to_string();
    }
    value.to_string()
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("secret")
        || lower.contains("authorization")
        || lower.contains("password")
}

fn truncate_json(value: &Value) -> Value {
    match value {
        Value::String(text) if text.len() > 8_192 => {
            Value::String(format!("{}...[truncated]", &text[..8_192]))
        }
        Value::Array(items) => Value::Array(items.iter().map(truncate_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), truncate_json(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn sanitized_environment() -> BTreeMap<String, String> {
    let mut env_out = BTreeMap::new();
    for key in [
        "TM_MIKU_BASE_URL",
        "TM_MIKU_BEARER_TOKEN",
        "TM_MIKU_TOKEN",
        "TM_MIKU_E2E_TIMEOUT_MS",
        "TM_E2E_REQUIRE_ARTIFACT",
        "TM_LLM_E2E_LIVE",
        "TM_E2E_SPEAKER_MODEL",
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_BASE_URL",
        "TM_OMP_ACP_ENABLED",
        "TM_DATABASE_URL",
    ] {
        if let Ok(value) = env::var(key) {
            let value = if is_secret_key(key) {
                "[REDACTED]".to_string()
            } else if value.trim().is_empty() {
                String::new()
            } else {
                redact_string(&value)
            };
            env_out.insert(key.to_string(), value);
        }
    }
    env_out
}

fn git_evidence() -> GitEvidence {
    let revision = command_stdout("git", &["rev-parse", "HEAD"]);
    let status_short = command_stdout("git", &["status", "--short"])
        .map(|text| text.lines().map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    GitEvidence {
        revision,
        dirty: !status_short.is_empty(),
        status_short,
    }
}

fn command_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn ndjson_file(path: impl AsRef<Path>) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path.as_ref())
        .with_context(|| format!("opening evidence ndjson {}", path.as_ref().display()))
}

fn write_ndjson<T: Serialize>(file: &mut File, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *file, value).context("encoding ndjson record")?;
    writeln!(file).context("terminating ndjson record")?;
    Ok(())
}

fn write_json_file(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating evidence directory {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(value).context("encoding evidence JSON")?;
    fs::write(path, json).with_context(|| format!("writing evidence JSON {}", path.display()))
}

fn write_transcript(root: &Path, transcript: &[String]) -> Result<()> {
    let mut text = transcript.join("\n");
    text.push('\n');
    fs::write(root.join("transcript.md"), text)
        .with_context(|| format!("writing transcript {}", root.display()))
}

fn write_report(root: &Path, manifest: &EvidenceManifest) -> Result<()> {
    let mut lines = Vec::new();
    lines.push("# TempestMiku E2E Evidence Report".to_string());
    lines.push(String::new());
    lines.push(format!(
        "- Status: **{}**",
        if manifest.ok { "PASS" } else { "FAIL" }
    ));
    lines.push(format!("- Started: {}", manifest.started_at));
    lines.push(format!("- Finished: {}", manifest.finished_at));
    lines.push(format!("- Command: `{}`", manifest.command));
    lines.push(String::new());
    lines.push("## Scenarios".to_string());
    for scenario in &manifest.scenarios {
        lines.push(format!(
            "- `{}`: {}",
            scenario.name,
            if scenario.ok { "PASS" } else { "FAIL" }
        ));
    }
    lines.push(String::new());
    lines.push("## Evidence Files".to_string());
    for path in [
        "manifest.json",
        "events.ndjson",
        "http.ndjson",
        "transcript.md",
    ] {
        lines.push(format!("- [{}]({})", path, path));
    }
    for resource in &manifest.resources {
        lines.push(format!(
            "- `{}`: [{}]({}), [{}]({})",
            resource.uri,
            resource.preview_path,
            resource.preview_path,
            resource.resolve_path,
            resource.resolve_path
        ));
    }
    if let Some(ui) = &manifest.ui {
        lines.push(String::new());
        lines.push("## UI".to_string());
        for path in [
            ui.result_path.as_deref(),
            ui.playwright_json_path.as_deref(),
            ui.screenshot_path.as_deref(),
            ui.console_path.as_deref(),
            ui.network_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            lines.push(format!("- [{}]({})", path, path));
        }
        for path in &ui.artifacts {
            lines.push(format!("- [{}]({})", path, path));
        }
    }
    lines.push(String::new());
    fs::write(root.join("report.md"), lines.join("\n"))
        .with_context(|| format!("writing report {}", root.display()))
}

fn write_index(root: &Path, manifest: &EvidenceManifest) -> Result<()> {
    let scenario_rows = manifest
        .scenarios
        .iter()
        .map(|scenario| {
            format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                escape_html(&scenario.name),
                if scenario.ok { "PASS" } else { "FAIL" },
                escape_html(scenario.error.as_deref().unwrap_or(""))
            )
        })
        .collect::<String>();
    let html = format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>TempestMiku E2E Evidence</title>
  <style>
    body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 32px; line-height: 1.5; }}
    code {{ background: #f2f4f7; padding: 2px 5px; border-radius: 4px; }}
    table {{ border-collapse: collapse; width: 100%; max-width: 920px; }}
    td, th {{ border-bottom: 1px solid #d0d7de; padding: 8px; text-align: left; }}
  </style>
</head>
<body>
  <h1>TempestMiku E2E Evidence</h1>
  <p>Status: <strong>{}</strong></p>
  <p>Started: <code>{}</code><br>Finished: <code>{}</code></p>
  <p><a href="report.md">Report</a> · <a href="manifest.json">Manifest</a> · <a href="events.ndjson">SSE events</a> · <a href="http.ndjson">HTTP log</a> · <a href="transcript.md">Transcript</a></p>
  <h2>Scenarios</h2>
  <table><thead><tr><th>Name</th><th>Status</th><th>Error</th></tr></thead><tbody>{}</tbody></table>
</body>
</html>
"#,
        if manifest.ok { "PASS" } else { "FAIL" },
        escape_html(&manifest.started_at),
        escape_html(&manifest.finished_at),
        scenario_rows
    );
    fs::write(root.join("index.html"), html)
        .with_context(|| format!("writing evidence index {}", root.display()))
}

fn validate_manifest_paths(root: &Path, manifest: &EvidenceManifest) -> Result<()> {
    for path in [
        "manifest.json",
        "events.ndjson",
        "http.ndjson",
        "transcript.md",
        "report.md",
        "index.html",
    ] {
        ensure!(
            root.join(path).exists(),
            "evidence file {path} was not written"
        );
    }
    for path in manifest.artifacts.values() {
        ensure!(
            root.join(path).exists(),
            "declared artifact {path} is missing"
        );
    }
    for resource in &manifest.resources {
        ensure!(
            root.join(&resource.preview_path).exists(),
            "declared resource preview {} is missing",
            resource.preview_path
        );
        ensure!(
            root.join(&resource.resolve_path).exists(),
            "declared resource resolve {} is missing",
            resource.resolve_path
        );
    }
    if let Some(ui) = &manifest.ui {
        for path in [
            ui.result_path.as_deref(),
            ui.playwright_json_path.as_deref(),
            ui.screenshot_path.as_deref(),
            ui.console_path.as_deref(),
            ui.network_path.as_deref(),
            ui.stdout_path.as_deref(),
            ui.stderr_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                root.join(path).exists(),
                "declared UI file {path} is missing"
            );
        }
        for path in &ui.artifacts {
            ensure!(
                root.join(path).exists(),
                "declared UI artifact {path} is missing"
            );
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn resource_slug(session_id: &str, uri: &str, index: usize) -> String {
    let mut slug = format!("{index:03}-{session_id}-{}", uri.replace("://", "-"));
    slug = slug
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    slug.trim_matches('-').to_string()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
