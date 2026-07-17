use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    common::{DriveEntryId, DriveOrganizerRunId, DriveRunId, initial_record_version},
    entry::DriveEvidence,
};

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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriveAutomationTier {
    #[default]
    Conservative,
    Moderate,
    Aggressive,
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
    #[serde(default = "initial_record_version")]
    pub version: u64,
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
    #[serde(default = "initial_record_version")]
    pub version: u64,
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
