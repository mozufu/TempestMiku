use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{common::DriveEntryId, initial_record_version, organizer::OrganizerProposal};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriveEntryStatus {
    #[default]
    Active,
    Archived,
    Deleted,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DriveCollisionStrategy {
    #[default]
    KeepBoth,
    Reject,
    Overwrite,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DriveApprovalMode {
    #[default]
    Propose,
    Auto,
    RequireApproval,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DriveDedupeMode {
    #[default]
    ContentHash,
    Off,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DrivePutOptions {
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub suggested_path: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub doc_kind: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub approval_mode: DriveApprovalMode,
    #[serde(default)]
    pub dedupe: DriveDedupeMode,
    #[serde(default)]
    pub collision: DriveCollisionStrategy,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub event_seq: Option<i64>,
    #[serde(default)]
    pub conventions: DriveConventions,
    #[serde(default)]
    pub model_extraction: DriveModelExtractionOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveModelExtractionOptions {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default = "default_model_extraction_fields")]
    pub fields: Vec<String>,
    #[serde(default = "default_model_extraction_preview_bytes")]
    pub max_preview_bytes: usize,
}

impl Default for DriveModelExtractionOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            role: None,
            fields: default_model_extraction_fields(),
            max_preview_bytes: default_model_extraction_preview_bytes(),
        }
    }
}

pub fn default_model_extraction_fields() -> Vec<String> {
    [
        "doc_kind",
        "entities",
        "dates",
        "amounts",
        "summary",
        "embedding",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn default_model_extraction_preview_bytes() -> usize {
    2_000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveModelExtractionRequest {
    pub role: String,
    pub fields: Vec<String>,
    pub mime: String,
    #[serde(default)]
    pub filename: Option<String>,
    pub content_hash: String,
    pub text_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DriveConventions {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub finance: Option<String>,
    #[serde(default)]
    pub inbox: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveSearchOptions {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub doc_kind: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub return_snippets: bool,
}

fn default_search_limit() -> usize {
    20
}

impl Default for DriveSearchOptions {
    fn default() -> Self {
        Self {
            query: None,
            project: None,
            doc_kind: None,
            tags: Vec::new(),
            limit: default_search_limit(),
            include_archived: false,
            since: None,
            until: None,
            return_snippets: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DriveListOptions {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default = "default_list_limit")]
    pub limit: usize,
    #[serde(default)]
    pub include_archived: bool,
}

fn default_list_limit() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveAmount {
    pub raw: String,
    pub value: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveEvidence {
    pub snippet: String,
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveAttribute {
    pub key: String,
    pub value: String,
    pub confidence: f32,
    #[serde(default)]
    pub evidence: Option<DriveEvidence>,
    pub extractor: String,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub event_seq: Option<i64>,
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveProvenance {
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub event_seq: Option<i64>,
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub source_run_id: Option<Uuid>,
    pub content_hash: String,
    pub extractor: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveEntry {
    pub id: DriveEntryId,
    #[serde(default = "initial_record_version")]
    pub version: u64,
    pub path: String,
    pub uri: String,
    pub blob_uri: String,
    pub content_hash: String,
    pub mime: String,
    pub size_bytes: usize,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub doc_kind: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub dates: Vec<String>,
    #[serde(default)]
    pub amounts: Vec<DriveAmount>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub embedding: Option<String>,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub provenance: Vec<DriveProvenance>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub status: DriveEntryStatus,
    #[serde(default)]
    pub attributes: Vec<DriveAttribute>,
    #[serde(default)]
    pub summary: Option<String>,
}

impl DriveEntry {
    pub fn drive_uri(path: &str) -> String {
        format!("drive://{path}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DrivePutResult {
    pub entry: DriveEntry,
    pub uri: String,
    pub proposed_path: String,
    pub filed: bool,
    #[serde(default)]
    pub proposal: Option<OrganizerProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveSearchResult {
    pub uri: String,
    pub path: String,
    pub title: Option<String>,
    pub doc_kind: Option<String>,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub content_hash: String,
    pub score: f32,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveVirtualQuery {
    pub kind: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub month: Option<u32>,
}
