use std::{
    collections::BTreeMap,
    fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::E2eEvent;

use super::{model::*, report::*, sanitize::*};

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
