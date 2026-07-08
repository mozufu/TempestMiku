use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type DriveEntryId = Uuid;
pub type DriveRunId = Uuid;
pub type DriveOrganizerRunId = Uuid;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DriveError {
    #[error("drive entry not found: {0}")]
    NotFound(String),
    #[error("invalid drive args: {0}")]
    InvalidArgs(String),
    #[error("invalid drive path: {0}")]
    InvalidPath(String),
    #[error("drive collision at {0}")]
    Collision(String),
    #[error("drive integrity check failed for {path}: expected {expected}, got {actual}")]
    Integrity {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("drive store error: {0}")]
    Store(String),
}

pub type Result<T, E = DriveError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriveEntryStatus {
    Active,
    Archived,
    Deleted,
}

impl Default for DriveEntryStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DriveCollisionStrategy {
    KeepBoth,
    Reject,
    Overwrite,
}

impl Default for DriveCollisionStrategy {
    fn default() -> Self {
        Self::KeepBoth
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DriveApprovalMode {
    Propose,
    Auto,
    RequireApproval,
}

impl Default for DriveApprovalMode {
    fn default() -> Self {
        Self::Propose
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DriveDedupeMode {
    ContentHash,
    Off,
}

impl Default for DriveDedupeMode {
    fn default() -> Self {
        Self::ContentHash
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl Default for DrivePutOptions {
    fn default() -> Self {
        Self {
            auto: false,
            suggested_path: None,
            project: None,
            doc_kind: None,
            tags: Vec::new(),
            source_uri: None,
            mime: None,
            title: None,
            approval_mode: DriveApprovalMode::default(),
            dedupe: DriveDedupeMode::default(),
            collision: DriveCollisionStrategy::default(),
            overwrite: false,
            session_id: None,
            event_seq: None,
            conventions: DriveConventions::default(),
            model_extraction: DriveModelExtractionOptions::default(),
        }
    }
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
#[serde(rename_all = "snake_case")]
pub enum OrganizerActionKind {
    Move,
    Tag,
    Dedupe,
    Archive,
    SetDocKind,
    SetProject,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriveAutomationTier {
    Conservative,
    Moderate,
    Aggressive,
}

impl Default for DriveAutomationTier {
    fn default() -> Self {
        Self::Conservative
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveOrganizerAutoApplyRule {
    #[serde(default)]
    pub actions: Vec<OrganizerActionKind>,
    #[serde(default)]
    pub doc_kinds: Vec<String>,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default = "default_organizer_auto_apply_min_confidence")]
    pub min_confidence: f32,
}

impl Default for DriveOrganizerAutoApplyRule {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            doc_kinds: Vec::new(),
            projects: Vec::new(),
            min_confidence: default_organizer_auto_apply_min_confidence(),
        }
    }
}

fn default_organizer_auto_apply_min_confidence() -> f32 {
    0.8
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DriveOrganizerConfig {
    #[serde(default)]
    pub tier: DriveAutomationTier,
    #[serde(default)]
    pub auto_apply: Vec<DriveOrganizerAutoApplyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Denied,
    Applied,
    Stale,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    AutoApply,
    ApprovalRequired,
    Denied,
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrganizerRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizerRun {
    pub id: DriveOrganizerRunId,
    pub trigger: String,
    pub status: OrganizerRunStatus,
    pub attempts: u32,
    pub proposal_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub available_at: DateTime<Utc>,
    #[serde(default)]
    pub locked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizerProposal {
    pub id: Uuid,
    pub action: OrganizerActionKind,
    pub entry_id: DriveEntryId,
    pub source_path: String,
    #[serde(default)]
    pub proposed_path: Option<String>,
    #[serde(default)]
    pub proposed_tags: Vec<String>,
    #[serde(default)]
    pub proposed_doc_kind: Option<String>,
    #[serde(default)]
    pub proposed_project: Option<String>,
    pub evidence: Vec<DriveEvidence>,
    pub confidence: f32,
    pub policy_decision: PolicyDecision,
    #[serde(default)]
    pub approval_id: Option<String>,
    pub status: ProposalStatus,
    pub source_run_id: DriveRunId,
    #[serde(default)]
    pub replay_metadata: BTreeMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveLinkPlan {
    pub alias: String,
    pub canonical_root: String,
    pub mode: String,
    pub linked_uri: String,
    pub memory_scope: String,
    pub project: String,
    #[serde(default)]
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveUnlinkResult {
    pub alias: String,
    pub canonical_root: String,
    pub linked_uri: String,
    pub memory_scope: String,
    pub revoked_at: DateTime<Utc>,
}

pub const DRIVE_EVENT_NAMES: &[&str] = &[
    "drive_put",
    "drive_transduced",
    "drive_path_proposed",
    "drive_write_proposed",
    "drive_filed",
    "drive_moved",
    "drive_tagged",
    "drive_linked",
    "drive_unlinked",
    "drive_organizer_started",
    "drive_organizer_completed",
    "drive_organizer_failed",
];

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn drive_payloads_use_stable_camel_case_wire_shapes() {
        let now = Utc::now();
        let entry = DriveEntry {
            id: Uuid::nil(),
            path: "projects/tempestmiku/notes/p5.txt".to_string(),
            uri: "drive://projects/tempestmiku/notes/p5.txt".to_string(),
            blob_uri: "blob:sha256:abc".to_string(),
            content_hash: "abc".to_string(),
            mime: "text/plain".to_string(),
            size_bytes: 3,
            title: Some("P5".to_string()),
            doc_kind: Some("note".to_string()),
            project: Some("TempestMiku".to_string()),
            entities: vec!["Tempest Miku".to_string()],
            dates: vec!["2026-07-08".to_string()],
            amounts: Vec::new(),
            tags: vec!["planning".to_string()],
            embedding: None,
            source_uri: Some("fixture://p5".to_string()),
            provenance: vec![DriveProvenance {
                source_uri: Some("fixture://p5".to_string()),
                session_id: Some("session".to_string()),
                event_seq: Some(7),
                actor_id: None,
                source_run_id: None,
                content_hash: "abc".to_string(),
                extractor: "test".to_string(),
                created_at: now,
            }],
            created_at: now,
            updated_at: now,
            status: DriveEntryStatus::Active,
            attributes: vec![DriveAttribute {
                key: "doc_kind".to_string(),
                value: "note".to_string(),
                confidence: 1.0,
                evidence: Some(DriveEvidence {
                    snippet: "P5".to_string(),
                    selector: Some("1-1".to_string()),
                }),
                extractor: "test".to_string(),
                source_uri: Some("fixture://p5".to_string()),
                session_id: Some("session".to_string()),
                event_seq: Some(7),
                content_hash: Some("abc".to_string()),
            }],
            summary: Some("P5".to_string()),
        };
        let proposal = OrganizerProposal {
            id: Uuid::nil(),
            action: OrganizerActionKind::Move,
            entry_id: entry.id,
            source_path: "inbox/p5.txt".to_string(),
            proposed_path: Some(entry.path.clone()),
            proposed_tags: vec!["planning".to_string()],
            proposed_doc_kind: Some("note".to_string()),
            proposed_project: Some("TempestMiku".to_string()),
            evidence: vec![DriveEvidence {
                snippet: "P5".to_string(),
                selector: Some("1-1".to_string()),
            }],
            confidence: 0.9,
            policy_decision: PolicyDecision::ApprovalRequired,
            approval_id: Some("approval-1".to_string()),
            status: ProposalStatus::Pending,
            source_run_id: Uuid::nil(),
            replay_metadata: [("contentHash".to_string(), json!("abc"))].into(),
            created_at: now,
            updated_at: now,
        };
        let put = DrivePutResult {
            entry: entry.clone(),
            uri: entry.uri.clone(),
            proposed_path: entry.path.clone(),
            filed: true,
            proposal: Some(proposal.clone()),
        };

        let value = serde_json::to_value(&put).unwrap();
        assert_eq!(
            value["proposedPath"],
            json!("projects/tempestmiku/notes/p5.txt")
        );
        assert_eq!(value["entry"]["blobUri"], json!("blob:sha256:abc"));
        assert_eq!(value["entry"]["contentHash"], json!("abc"));
        assert_eq!(value["entry"]["createdAt"].is_string(), true);
        assert_eq!(
            value["proposal"]["policyDecision"],
            json!("approval_required")
        );
        assert_eq!(value["proposal"]["sourceRunId"].is_string(), true);

        let config = DriveOrganizerConfig {
            tier: DriveAutomationTier::Moderate,
            auto_apply: vec![DriveOrganizerAutoApplyRule {
                actions: vec![OrganizerActionKind::Move],
                doc_kinds: vec!["note".to_string()],
                projects: vec!["TempestMiku".to_string()],
                min_confidence: 0.7,
            }],
        };
        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["tier"], json!("moderate"));
        assert_eq!(value["autoApply"][0]["actions"][0], json!("move"));
        assert_eq!(value["autoApply"][0]["docKinds"][0], json!("note"));
        assert_eq!(value["autoApply"][0]["projects"][0], json!("TempestMiku"));
        assert!((value["autoApply"][0]["minConfidence"].as_f64().unwrap() - 0.7).abs() < 1e-6);

        let run = OrganizerRun {
            id: Uuid::nil(),
            trigger: "manual".to_string(),
            status: OrganizerRunStatus::Running,
            attempts: 2,
            proposal_ids: vec![proposal.id],
            created_at: now,
            available_at: now,
            locked_at: Some(now),
            completed_at: None,
            last_error: None,
        };
        let value = serde_json::to_value(run).unwrap();
        assert_eq!(value["proposalIds"][0], json!(Uuid::nil()));
        assert_eq!(value["lockedAt"].is_string(), true);
        assert_eq!(value["completedAt"], json!(null));

        let search = DriveSearchResult {
            uri: entry.uri,
            path: entry.path,
            title: entry.title,
            doc_kind: entry.doc_kind,
            project: entry.project,
            tags: entry.tags,
            content_hash: entry.content_hash,
            score: 1.0,
            snippet: Some("P5".to_string()),
            selector: Some("1-1".to_string()),
        };
        let value = serde_json::to_value(search).unwrap();
        assert_eq!(value["docKind"], json!("note"));
        assert_eq!(value["contentHash"], json!("abc"));
    }

    #[test]
    fn drive_event_names_are_reserved_in_order() {
        assert_eq!(
            DRIVE_EVENT_NAMES,
            [
                "drive_put",
                "drive_transduced",
                "drive_path_proposed",
                "drive_write_proposed",
                "drive_filed",
                "drive_moved",
                "drive_tagged",
                "drive_linked",
                "drive_unlinked",
                "drive_organizer_started",
                "drive_organizer_completed",
                "drive_organizer_failed",
            ]
        );
    }
}
