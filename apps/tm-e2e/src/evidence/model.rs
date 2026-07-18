use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
