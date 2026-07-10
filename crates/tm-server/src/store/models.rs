use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_memory::{
    DreamLease, DreamQueueRecord, MemoryEvidenceRef, MemorySummaryRecord, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, ProfileFactRecord, RecallChunkRecord,
    SkillProposalRecord, SkillProposalStatus,
};
use tm_modes::{AssetStatus, ModeId};
use uuid::Uuid;

use crate::{Result, ServerError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
    pub mode: ModeId,
    pub mode_state: ModeState,
    pub persona_status: AssetStatus,
    #[serde(default = "default_owner_subject")]
    pub owner_subject: String,
    #[serde(default = "default_memory_scope")]
    pub memory_scope: String,
}

fn default_owner_subject() -> String {
    "brian".to_string()
}

fn default_memory_scope() -> String {
    "global".to_string()
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub mode: ModeId,
    pub persona_status: AssetStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModeState {
    pub mode: ModeId,
    pub router_reason: Option<String>,
    pub lock_source: Option<String>,
    pub override_source: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl ModeState {
    pub fn new(mode: ModeId, updated_at: DateTime<Utc>) -> Self {
        Self {
            mode,
            router_reason: None,
            lock_source: None,
            override_source: None,
            updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEvent {
    pub session_id: Uuid,
    pub seq: i64,
    pub event_type: String,
    pub payload_json: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl SessionEvent {
    pub fn new(
        session_id: Uuid,
        seq: i64,
        event_type: impl Into<String>,
        mut payload_json: Value,
        created_at: DateTime<Utc>,
    ) -> Self {
        tm_memory::redact_json_value(&mut payload_json);
        let event_type = event_type.into();
        let refs = SessionEventOutputRefs::from_payload(&event_type, &payload_json);
        Self {
            session_id,
            seq,
            event_type,
            payload_json,
            actor_id: refs.actor_id,
            artifact_uri: refs.artifact_uri,
            history_uri: refs.history_uri,
            turn_id: None,
            created_at,
        }
    }

    pub(crate) fn output_refs(event_type: &str, payload_json: &Value) -> SessionEventOutputRefs {
        SessionEventOutputRefs::from_payload(event_type, payload_json)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionEventOutputRefs {
    pub actor_id: Option<String>,
    pub artifact_uri: Option<String>,
    pub history_uri: Option<String>,
}

impl SessionEventOutputRefs {
    fn from_payload(event_type: &str, payload_json: &Value) -> Self {
        if !matches!(event_type, "actor_completed" | "actor_resources_linked") {
            return Self::default();
        }

        let artifact_uri = payload_string(payload_json, "artifact_uri", "artifactUri");
        let history_uri = payload_string(payload_json, "history_uri", "historyUri");
        if artifact_uri.is_none() && history_uri.is_none() {
            return Self::default();
        }

        Self {
            actor_id: payload_string(payload_json, "actor_id", "actorId"),
            artifact_uri,
            history_uri,
        }
    }
}

fn payload_string(payload_json: &Value, snake_key: &str, camel_key: &str) -> Option<String> {
    payload_json
        .get(snake_key)
        .or_else(|| payload_json.get(camel_key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRecord {
    pub session_id: Uuid,
    pub seq: i64,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurnRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub client_message_id: String,
    pub content: String,
    pub content_hash: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub worker_id: Option<Uuid>,
    pub error: Option<String>,
}

pub(crate) fn turn_content_hash(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

pub(crate) fn redact_persisted_text(content: &str) -> String {
    tm_memory::redact_dream_text(content).text
}

pub(crate) fn redact_persisted_json(mut payload: Value) -> Value {
    tm_memory::redact_json_value(&mut payload);
    payload
}

pub(crate) fn validate_profile_fact_persistence(fact: &ProfileFactRecord) -> Result<()> {
    reject_sensitive_persistence_fields([
        ("profile fact subject", fact.subject.as_str()),
        ("profile fact predicate", fact.predicate.as_str()),
        ("profile fact object", fact.object.as_str()),
        ("profile fact provenance", fact.provenance.as_str()),
    ])
}

pub(crate) fn validate_recall_chunk_persistence(chunk: &RecallChunkRecord) -> Result<()> {
    reject_sensitive_persistence_fields([
        ("recall scope", chunk.scope.as_str()),
        ("recall text", chunk.text.as_str()),
        ("recall source", chunk.source.as_str()),
    ])
}

pub(crate) fn validate_persistence_identifier(field: &str, value: &str) -> Result<()> {
    reject_sensitive_persistence_fields([(field, value)])
}

pub(crate) fn sanitize_project_item_persistence(
    mut item: NewProjectItem,
) -> Result<NewProjectItem> {
    reject_sensitive_persistence_fields([
        ("project id", item.project_id.as_str()),
        ("project target URI", item.target_uri.as_str()),
        ("project dedupe key", item.dedupe_key.as_str()),
    ])?;
    if let Some(source_uri) = item.source_uri.as_deref() {
        reject_sensitive_persistence_fields([("project source URI", source_uri)])?;
    }
    item.text = redact_persisted_text(&item.text);
    item.provenance_json = redact_persisted_json(item.provenance_json);
    Ok(item)
}

pub(crate) fn sanitize_memory_summary_persistence(
    mut summary: NewMemorySummaryRecord,
) -> Result<NewMemorySummaryRecord> {
    reject_sensitive_persistence_fields([
        ("memory summary subject", summary.subject.as_str()),
        ("memory summary scope", summary.scope.as_str()),
        ("memory summary dedupe key", summary.dedupe_key.as_str()),
    ])?;
    summary.title = redact_persisted_text(&summary.title);
    summary.body = redact_persisted_text(&summary.body);
    sanitize_memory_evidence(&mut summary.evidence)?;
    Ok(summary)
}

pub(crate) fn sanitize_skill_proposal_persistence(
    mut proposal: NewSkillProposalRecord,
) -> Result<NewSkillProposalRecord> {
    reject_sensitive_persistence_fields([
        ("skill proposal name", proposal.name.as_str()),
        ("skill proposal dedupe key", proposal.dedupe_key.as_str()),
    ])?;
    proposal.description = redact_persisted_text(&proposal.description);
    proposal.body = redact_persisted_text(&proposal.body);
    proposal.trigger = redact_persisted_text(&proposal.trigger);
    proposal.use_criteria = redact_persisted_text(&proposal.use_criteria);
    proposal.self_critique = redact_persisted_text(&proposal.self_critique);
    proposal.verification.checks = std::mem::take(&mut proposal.verification.checks)
        .into_iter()
        .map(|check| redact_persisted_text(&check))
        .collect();
    sanitize_memory_evidence(&mut proposal.evidence)?;
    Ok(proposal)
}

pub(crate) fn sanitize_cron_job_persistence(mut job: NewCronJobRecord) -> Result<NewCronJobRecord> {
    reject_sensitive_persistence_fields([
        ("cron job id", job.id.as_str()),
        ("cron schedule", job.schedule.as_str()),
        ("cron mode", job.cron_mode.as_str()),
    ])?;
    job.name = redact_persisted_text(&job.name);
    Ok(job)
}

pub(crate) fn sanitize_cron_run_persistence(mut run: NewCronRunRecord) -> Result<NewCronRunRecord> {
    reject_sensitive_persistence_fields([
        ("cron run job id", run.job_id.as_str()),
        ("cron run status", run.status.as_str()),
    ])?;
    run.result_json = redact_persisted_json(run.result_json);
    Ok(run)
}

fn sanitize_memory_evidence(evidence: &mut [MemoryEvidenceRef]) -> Result<()> {
    for item in evidence {
        if let Some(uri) = item.uri.as_deref() {
            reject_sensitive_persistence_fields([("memory evidence URI", uri)])?;
        }
        item.label = redact_persisted_text(&item.label);
    }
    Ok(())
}

fn reject_sensitive_persistence_fields<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<()> {
    for (field, value) in fields {
        if tm_memory::contains_sensitive_data(value) {
            return Err(ServerError::InvalidRequest(format!(
                "{field} contains sensitive data"
            )));
        }
    }
    Ok(())
}

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

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord>;
    async fn configure_owner_subject(&self, owner_subject: &str) -> Result<usize>;
    async fn set_session_memory_scope(
        &self,
        session_id: Uuid,
        memory_scope: &str,
    ) -> Result<SessionRecord>;
    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord>;
    async fn end_session_and_enqueue_dream(
        &self,
        session_id: Uuid,
        subject: String,
        scope: String,
    ) -> Result<EndSessionDreamResult>;
    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummaryRecord>>;
    async fn get_session(&self, session_id: Uuid) -> Result<SessionRecord>;
    async fn session_messages(&self, session_id: Uuid) -> Result<Vec<MessageRecord>>;
    async fn set_mode_state(
        &self,
        session_id: Uuid,
        mode_state: ModeState,
    ) -> Result<SessionRecord>;
    async fn append_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<MessageRecord>;
    async fn append_message_for_turn(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        turn_id: Option<Uuid>,
    ) -> Result<MessageRecord> {
        if turn_id.is_some() {
            return Err(ServerError::InvalidRequest(
                "store does not support turn-linked messages".to_string(),
            ));
        }
        self.append_message(session_id, role, content).await
    }
    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent>;
    async fn append_event_for_turn(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
        turn_id: Option<Uuid>,
    ) -> Result<SessionEvent> {
        if turn_id.is_some() {
            return Err(ServerError::InvalidRequest(
                "store does not support turn-linked events".to_string(),
            ));
        }
        self.append_event(session_id, event_type, payload_json)
            .await
    }
    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>>;
    async fn enqueue_turn(
        &self,
        session_id: Uuid,
        client_message_id: &str,
        content: &str,
    ) -> Result<SessionTurnRecord>;
    async fn turn(&self, turn_id: Uuid) -> Result<SessionTurnRecord>;
    async fn claim_next_turn(
        &self,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionTurnRecord>>;
    async fn heartbeat_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<SessionTurnRecord>;
    async fn fail_stale_running_turns(
        &self,
        stale_before: DateTime<Utc>,
        failed_at: DateTime<Utc>,
        error: &str,
    ) -> Result<usize>;
    async fn complete_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        assistant_content: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord>;
    async fn fail_turn(
        &self,
        turn_id: Uuid,
        worker_id: Uuid,
        error: &str,
        failed_at: DateTime<Utc>,
    ) -> Result<SessionTurnRecord>;
    async fn create_approval_request(
        &self,
        request: NewApprovalRequest,
    ) -> Result<ApprovalRequestRecord>;
    async fn approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
    ) -> Result<ApprovalRequestRecord>;
    async fn heartbeat_approval_request(
        &self,
        approval_id: Uuid,
        requester_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequestRecord>;
    async fn resolve_approval_request(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        resolution: NewApprovalResolution,
    ) -> Result<ApprovalRequestRecord>;
    async fn resolve_approval_request_with_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        resolution: NewApprovalResolution,
    ) -> Result<(ApprovalRequestRecord, SessionEvent)>;
    async fn link_approval_event(
        &self,
        session_id: Uuid,
        approval_id: Uuid,
        event_type: &str,
        event_seq: i64,
    ) -> Result<ApprovalRequestRecord>;
    async fn cancel_stale_non_resumable_approvals(
        &self,
        stale_before: DateTime<Utc>,
        cancelled_at: DateTime<Utc>,
    ) -> Result<Vec<SessionEvent>>;
    async fn expire_pending_approvals(&self, now: DateTime<Utc>) -> Result<Vec<SessionEvent>>;
    async fn claim_approval_effect(
        &self,
        approval_id: Uuid,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>>;
    async fn claim_next_approval_effect(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<Option<ApprovalEffectLease>>;
    async fn complete_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        applied_at: DateTime<Utc>,
    ) -> Result<ApprovalEffectRecord>;
    async fn complete_approval_effect_with_event(
        &self,
        lease: &ApprovalEffectLease,
        proposal_payload_json: Value,
        turn_id: Option<Uuid>,
        applied_at: DateTime<Utc>,
    ) -> Result<(ApprovalEffectRecord, SessionEvent)>;
    async fn fail_approval_effect(
        &self,
        lease: &ApprovalEffectLease,
        error: &str,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<ApprovalEffectRecord>;
    async fn add_profile_fact(&self, fact: ProfileFactRecord) -> Result<()>;
    async fn add_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<()>;
    async fn upsert_profile_fact(&self, fact: ProfileFactRecord) -> Result<ProfileFactRecord>;
    async fn upsert_recall_chunk(&self, chunk: RecallChunkRecord) -> Result<RecallChunkRecord>;
    async fn profile_facts(&self, subject: &str) -> Result<Vec<ProfileFactRecord>>;
    async fn profile_fact(&self, subject: &str, id: Uuid) -> Result<ProfileFactRecord>;
    async fn recall_chunks(
        &self,
        scope: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallChunkRecord>>;
    async fn recall_chunk(&self, scope: &str, id: Uuid) -> Result<RecallChunkRecord>;
    async fn upsert_project_item(&self, item: NewProjectItem) -> Result<ProjectItemRecord>;
    async fn project_items(
        &self,
        project_id: &str,
        kind: Option<ProjectItemKind>,
    ) -> Result<Vec<ProjectItemRecord>>;
    async fn enqueue_dream(&self, new: NewDreamQueueRecord) -> Result<DreamQueueRecord>;
    async fn dream_queue_for_session(&self, session_id: Uuid) -> Result<Vec<DreamQueueRecord>>;
    async fn dream_queue(&self, scope: &str, limit: usize) -> Result<Vec<DreamQueueRecord>>;
    async fn dream(&self, dream_id: Uuid) -> Result<DreamQueueRecord>;
    async fn claim_ready_dream(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
    ) -> Result<Option<DreamLease>>;
    async fn claim_ready_dream_bounded(
        &self,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        owner_id: Uuid,
        max_attempts: i32,
    ) -> Result<Option<DreamLease>> {
        let _ = max_attempts;
        self.claim_ready_dream(now, lease_timeout, owner_id).await
    }
    async fn heartbeat_dream(&self, lease: &DreamLease, now: DateTime<Utc>) -> Result<DreamLease>;
    async fn complete_dream(
        &self,
        lease: &DreamLease,
        now: DateTime<Utc>,
    ) -> Result<DreamQueueRecord>;
    async fn fail_dream(
        &self,
        lease: &DreamLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<DreamQueueRecord>;
    async fn upsert_memory_summary(
        &self,
        summary: NewMemorySummaryRecord,
    ) -> Result<MemorySummaryRecord>;
    async fn memory_summary(&self, id: Uuid) -> Result<MemorySummaryRecord>;
    async fn memory_summaries(&self, scope: &str, limit: usize)
    -> Result<Vec<MemorySummaryRecord>>;
    async fn upsert_skill_proposal(
        &self,
        proposal: NewSkillProposalRecord,
    ) -> Result<SkillProposalRecord>;
    async fn update_skill_proposal_status(
        &self,
        id: Uuid,
        status: SkillProposalStatus,
    ) -> Result<SkillProposalRecord>;
    async fn skill_proposal(&self, id: Uuid) -> Result<SkillProposalRecord>;
    async fn skill_proposals_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<SkillProposalRecord>>;
    async fn upsert_cron_job(&self, job: NewCronJobRecord) -> Result<CronJobRecord>;
    async fn cron_job(&self, id: &str) -> Result<CronJobRecord>;
    async fn cron_jobs(&self) -> Result<Vec<CronJobRecord>>;
    /// Atomically materialize one scheduled fire and advance the job cursor.
    ///
    /// Returns `None` when another scheduler already advanced the expected cursor.
    async fn materialize_cron_run(
        &self,
        run: NewCronRunRecord,
        expected_next_run_at: DateTime<Utc>,
        next_run_at: DateTime<Utc>,
    ) -> Result<Option<CronRunRecord>>;
    /// Claim the oldest ready or stale cron run independently from materialization.
    async fn claim_ready_cron_run(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
        max_attempts: i32,
    ) -> Result<Option<CronLease>>;
    async fn claim_cron_run(
        &self,
        run: NewCronRunRecord,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: Duration,
    ) -> Result<(CronLease, bool)>;
    async fn record_cron_run(&self, run: NewCronRunRecord) -> Result<CronRunRecord>;
    async fn heartbeat_cron_run(&self, lease: &CronLease, now: DateTime<Utc>) -> Result<CronLease>;
    async fn complete_cron_run(
        &self,
        lease: &CronLease,
        status: &str,
        session_id: Option<Uuid>,
        result_json: Value,
    ) -> Result<CronRunRecord>;
    async fn fail_cron_run(
        &self,
        lease: &CronLease,
        error: String,
        next_available_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<CronRunRecord>;
    async fn cron_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>>;
    async fn runtime_metrics(&self, now: DateTime<Utc>) -> Result<StoreRuntimeMetrics>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StoreEvent {
    Text { delta: String },
    ModeChanged(Box<ModeChangedStoreEvent>),
    Final { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeChangedStoreEvent {
    pub from: Option<ModeId>,
    pub mode: ModeId,
    pub label: String,
    pub voice_cap: String,
    pub capabilities: Vec<String>,
    #[serde(rename = "activeSkills")]
    pub active_skills: Vec<String>,
    pub router_reason: Option<String>,
    pub lock_source: Option<String>,
    pub override_source: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub persona_status: AssetStatus,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tm_modes::{AssetStatus, ModeId};

    use super::*;

    #[test]
    fn mode_state_new_sets_mode_and_empty_sources() {
        let updated_at = Utc::now();
        let state = ModeState::new(ModeId::from("serious_engineer"), updated_at);

        assert_eq!(state.mode.as_str(), "serious_engineer");
        assert_eq!(state.router_reason, None);
        assert_eq!(state.lock_source, None);
        assert_eq!(state.override_source, None);
        assert_eq!(state.updated_at, updated_at);
    }

    #[test]
    fn mode_state_serializes_public_fields_as_camel_case() {
        let updated_at = Utc::now();
        let state = ModeState {
            mode: ModeId::from("general"),
            router_reason: Some("user asked directly".to_string()),
            lock_source: Some("user".to_string()),
            override_source: Some("manual".to_string()),
            updated_at,
        };

        let value = serde_json::to_value(&state).expect("serialize mode state");

        assert_eq!(value["mode"], json!("general"));
        assert_eq!(value["routerReason"], json!("user asked directly"));
        assert_eq!(value["lockSource"], json!("user"));
        assert_eq!(value["overrideSource"], json!("manual"));
        assert!(value.get("updatedAt").is_some());
        assert!(value.get("router_reason").is_none());
    }

    #[test]
    fn project_item_kind_round_trips_all_store_values() {
        let cases = [
            (ProjectItemKind::Status, "status"),
            (ProjectItemKind::Summary, "summary"),
            (ProjectItemKind::OpenLoop, "open_loop"),
            (ProjectItemKind::Decision, "decision"),
            (ProjectItemKind::NextAction, "next_action"),
            (ProjectItemKind::Resource, "resource"),
            (ProjectItemKind::Artifact, "artifact"),
            (ProjectItemKind::Workspace, "workspace"),
            (ProjectItemKind::Linked, "linked"),
        ];

        for (kind, wire) in cases {
            assert_eq!(kind.as_str(), wire);
            assert_eq!(wire.parse::<ProjectItemKind>().expect("parse kind"), kind);
            assert_eq!(
                serde_json::to_value(kind).expect("serialize kind"),
                json!(wire)
            );
        }
    }

    #[test]
    fn project_item_kind_rejects_unknown_values() {
        let err = "blocked".parse::<ProjectItemKind>().unwrap_err();

        assert!(matches!(err, ServerError::Store(_)));
        assert!(
            err.to_string()
                .contains("unknown project item kind blocked")
        );
    }

    #[test]
    fn store_event_serializes_sse_payload_shapes() {
        let updated_at = Utc::now();

        assert_eq!(
            serde_json::to_value(StoreEvent::Text {
                delta: "hello".to_string(),
            })
            .expect("serialize text event"),
            json!({"event": "text", "delta": "hello"})
        );
        assert_eq!(
            serde_json::to_value(StoreEvent::Final {
                text: "done".to_string(),
            })
            .expect("serialize final event"),
            json!({"event": "final", "text": "done"})
        );

        let value =
            serde_json::to_value(StoreEvent::ModeChanged(Box::new(ModeChangedStoreEvent {
                from: Some(ModeId::from("general")),
                mode: ModeId::from("serious_engineer"),
                label: "Serious Engineer".to_string(),
                voice_cap: "off".to_string(),
                capabilities: vec!["fs.read".to_string()],
                active_skills: vec!["serious-engineer-ops".to_string()],
                router_reason: Some("coding request".to_string()),
                lock_source: Some("user".to_string()),
                override_source: None,
                updated_at,
                persona_status: AssetStatus::Degraded {
                    warning: "missing configured assets".to_string(),
                },
            })))
            .expect("serialize mode changed event");

        assert_eq!(value["event"], json!("mode_changed"));
        assert_eq!(value["from"], json!("general"));
        assert_eq!(value["mode"], json!("serious_engineer"));
        assert_eq!(value["voice_cap"], json!("off"));
        assert_eq!(value["router_reason"], json!("coding request"));
        assert_eq!(value["lock_source"], json!("user"));
        assert_eq!(value["override_source"], json!(null));
        assert_eq!(value["activeSkills"], json!(["serious-engineer-ops"]));
        assert!(value.get("active_skills").is_none());
        assert_eq!(
            value["persona_status"],
            json!({"state": "degraded", "warning": "missing configured assets"})
        );
        assert!(value.get("updated_at").is_some());
    }
}
