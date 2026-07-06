use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRecord {
    pub session_id: Uuid,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileFactRecord {
    pub id: Uuid,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
    pub provenance: String,
    pub valid_from: DateTime<Utc>,
    pub valid_to: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecallChunkRecord {
    pub id: Uuid,
    pub scope: String,
    pub text: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
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

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn create_session(&self, new: NewSession) -> Result<SessionRecord>;
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
    async fn append_event(
        &self,
        session_id: Uuid,
        event_type: &str,
        payload_json: Value,
    ) -> Result<SessionEvent>;
    async fn events_after(
        &self,
        session_id: Uuid,
        last_event_id: Option<i64>,
    ) -> Result<Vec<SessionEvent>>;
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StoreEvent {
    Text {
        delta: String,
    },
    ModeChanged {
        from: Option<ModeId>,
        mode: ModeId,
        label: String,
        voice_cap: String,
        capabilities: Vec<String>,
        #[serde(rename = "activeSkills")]
        active_skills: Vec<String>,
        router_reason: Option<String>,
        lock_source: Option<String>,
        override_source: Option<String>,
        updated_at: DateTime<Utc>,
        persona_status: AssetStatus,
    },
    Final {
        text: String,
    },
}
