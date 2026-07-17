use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_host::EvolutionAuditRecord;
use tm_memory::{
    DenseRecallQuery, DreamLease, DreamQueueRecord, HybridRecallRequest, HybridRecallResult,
    MemoryEmbeddingGeneration, MemoryEmbeddingJobRecord, MemoryEvidenceRef, MemoryEvidenceSource,
    MemoryRecordKind, MemoryRecordResource, MemoryScopeTombstone, MemorySummaryRecord,
    NewDreamQueueRecord, NewMemoryEmbeddingJob, NewMemorySummaryRecord, NewSkillProposalRecord,
    ProfileFactRecord, RecallChunkRecord, SkillProposalRecord, SkillProposalStatus,
    StoredMemoryRecord,
};
use tm_modes::{
    AssetStatus, ModeId, ReviewAddendumChange, ReviewApplyContract, ReviewProposalStatus,
    ReviewProposalTarget,
};
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

mod contract;
mod events;
mod operations;
mod persistence;

pub use contract::Store;
pub use events::{ModeChangedStoreEvent, StoreEvent};
pub use operations::*;
pub(crate) use persistence::*;

#[cfg(test)]
mod tests;
