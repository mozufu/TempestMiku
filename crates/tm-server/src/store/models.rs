use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_memory::{DreamQueueRecord, NewDreamQueueRecord};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_uri: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl SessionEvent {
    pub fn new(
        session_id: Uuid,
        seq: i64,
        event_type: impl Into<String>,
        payload_json: Value,
        created_at: DateTime<Utc>,
    ) -> Self {
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
    async fn end_session(&self, session_id: Uuid) -> Result<SessionRecord>;
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
    async fn enqueue_dream(&self, new: NewDreamQueueRecord) -> Result<DreamQueueRecord>;
    async fn dream_queue_for_session(&self, session_id: Uuid) -> Result<Vec<DreamQueueRecord>>;
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

        let value = serde_json::to_value(StoreEvent::ModeChanged {
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
        })
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
