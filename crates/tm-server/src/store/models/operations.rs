use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub requester_id: Uuid,
    pub origin: String,
    pub action: String,
    pub scope_json: Value,
    pub options_json: Value,
    pub status: String,
    pub resumable: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub selected_option_id: Option<String>,
    pub resolution_json: Option<Value>,
    pub request_event_seq: Option<i64>,
    pub resolution_event_seq: Option<i64>,
    pub resolution_version: i64,
}

#[derive(Debug, Clone)]
pub struct NewApprovalRequest {
    pub id: Uuid,
    pub session_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub requester_id: Uuid,
    pub origin: String,
    pub action: String,
    pub scope_json: Value,
    pub options_json: Value,
    pub effect_type: String,
    pub effect_payload_json: Value,
    pub resumable: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewApprovalResolution {
    pub status: String,
    pub selected_option_id: Option<String>,
    pub resolution_json: Value,
    pub resolved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionAuditEntry {
    pub idempotency_key: String,
    pub record: EvolutionAuditRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionReviewProposalRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub target: ReviewProposalTarget,
    pub base_version: u64,
    pub base_digest: String,
    #[serde(default)]
    pub base_active_digest: Option<String>,
    pub changes: Vec<ReviewAddendumChange>,
    pub content_digest: String,
    pub status: ReviewProposalStatus,
    pub apply_contract: ReviewApplyContract,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_candidate: Option<PersonaAutoCandidate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewEvolutionReviewProposal {
    pub id: Uuid,
    pub session_id: Uuid,
    pub target: ReviewProposalTarget,
    pub base_version: u64,
    pub base_digest: String,
    pub base_active_digest: Option<String>,
    pub changes: Vec<ReviewAddendumChange>,
    pub content_digest: String,
    pub apply_contract: ReviewApplyContract,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_candidate: Option<PersonaAutoCandidate>,
}

pub const PERSONA_AUTO_CANDIDATE_SCHEMA_VERSION: u16 = 1;
pub const MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE: usize = 3;
pub const MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REFS: usize = 4;
pub const MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REF_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PersonaAutoCandidateTrigger {
    RepeatedPreference,
    PersonaMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonaAutoCandidateEvidence {
    pub record_id: Uuid,
    pub kind: tm_memory::MemoryRecordKind,
    pub source_uri: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonaAutoCandidate {
    pub schema_version: u16,
    pub trigger: PersonaAutoCandidateTrigger,
    pub dedupe_key: String,
    pub source_turn_id: Uuid,
    pub evidence: Vec<PersonaAutoCandidateEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoEvolutionReviewDisposition {
    Created,
    Duplicate,
    Cooldown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoEvolutionReviewProposalResult {
    pub disposition: AutoEvolutionReviewDisposition,
    pub proposal: EvolutionReviewProposalRecord,
}

#[derive(Debug, Clone)]
pub struct NewAutoEvolutionReviewBundle {
    pub proposal: NewEvolutionReviewProposal,
    pub approval: NewApprovalRequest,
    pub proposal_payload_json: Value,
    pub approval_payload_json: Value,
    pub cooldown_since: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoEvolutionReviewBundleResult {
    pub disposition: AutoEvolutionReviewDisposition,
    pub proposal: EvolutionReviewProposalRecord,
    pub approval: Option<ApprovalRequestRecord>,
    pub events: Vec<SessionEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalEffectRecord {
    pub id: Uuid,
    pub approval_id: Uuid,
    pub session_id: Uuid,
    pub effect_type: String,
    pub payload_json: Value,
    pub status: String,
    pub attempts: i32,
    pub available_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub lease_owner: Option<Uuid>,
    pub lease_epoch: i64,
    pub applied_at: Option<DateTime<Utc>>,
    pub error_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalEffectLease {
    pub effect: ApprovalEffectRecord,
    pub owner_id: Uuid,
    pub epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryRecord {
    pub session: SessionRecord,
    pub summary: Option<String>,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub message_count: i64,
    pub last_event_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProjectItemKind {
    Status,
    Summary,
    OpenLoop,
    Decision,
    NextAction,
    Resource,
    Artifact,
    Workspace,
    Linked,
}

impl ProjectItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Summary => "summary",
            Self::OpenLoop => "open_loop",
            Self::Decision => "decision",
            Self::NextAction => "next_action",
            Self::Resource => "resource",
            Self::Artifact => "artifact",
            Self::Workspace => "workspace",
            Self::Linked => "linked",
        }
    }
}

impl std::str::FromStr for ProjectItemKind {
    type Err = ServerError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "status" => Ok(Self::Status),
            "summary" => Ok(Self::Summary),
            "open_loop" => Ok(Self::OpenLoop),
            "decision" => Ok(Self::Decision),
            "next_action" => Ok(Self::NextAction),
            "resource" => Ok(Self::Resource),
            "artifact" => Ok(Self::Artifact),
            "workspace" => Ok(Self::Workspace),
            "linked" => Ok(Self::Linked),
            other => Err(ServerError::Store(format!(
                "unknown project item kind {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectItemRecord {
    pub id: Uuid,
    pub project_id: String,
    pub kind: ProjectItemKind,
    pub text: String,
    pub target_uri: String,
    pub source_session_id: Uuid,
    pub source_event_seq: Option<i64>,
    pub source_uri: Option<String>,
    pub dedupe_key: String,
    pub provenance_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewProjectItem {
    pub project_id: String,
    pub kind: ProjectItemKind,
    pub text: String,
    pub target_uri: String,
    pub source_session_id: Uuid,
    pub source_event_seq: Option<i64>,
    pub source_uri: Option<String>,
    pub dedupe_key: String,
    pub provenance_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CronJobRecord {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub enabled: bool,
    pub cron_mode: String,
    pub max_turns: i32,
    pub script_timeout_seconds: i32,
    pub next_run_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCronJobRecord {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub enabled: bool,
    pub cron_mode: String,
    pub max_turns: i32,
    pub script_timeout_seconds: i32,
    pub next_run_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CronRunRecord {
    pub id: Uuid,
    pub job_id: String,
    pub scheduled_for: DateTime<Utc>,
    pub status: String,
    pub session_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub available_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub lease_owner: Option<Uuid>,
    pub lease_epoch: i64,
    pub last_error: Option<String>,
    pub result_json: Value,
}

#[derive(Debug, Clone)]
pub struct NewCronRunRecord {
    pub job_id: String,
    pub scheduled_for: DateTime<Utc>,
    pub status: String,
    pub session_id: Option<Uuid>,
    pub result_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CronLease {
    pub run: CronRunRecord,
    pub owner_id: Uuid,
    pub epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoreRuntimeMetrics {
    pub turn_queue_depth: i64,
    pub turn_oldest_age_seconds: i64,
    pub dream_queue_depth: i64,
    pub dream_oldest_age_seconds: i64,
    pub cron_queue_depth: i64,
    pub cron_oldest_age_seconds: i64,
    pub scheduler_lag_seconds: i64,
    pub pending_approvals: i64,
    pub approval_effect_queue_depth: i64,
    pub approval_effect_oldest_age_seconds: i64,
    pub lease_reclaims: i64,
}

#[derive(Debug, Clone)]
pub struct EndSessionDreamResult {
    pub session: SessionRecord,
    pub dream: DreamQueueRecord,
    pub events: Vec<SessionEvent>,
    pub newly_ended: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Archived,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

impl std::str::FromStr for ProjectStatus {
    type Err = ServerError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            other => Err(ServerError::Store(format!(
                "unknown project status {other}"
            ))),
        }
    }
}

/// A server-owned project entity (§30). Its `id` is the canonical `project:<id>` memory-scope slug;
/// linked folders attach to it by id but are not part of the record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub title: String,
    pub status: ProjectStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}
