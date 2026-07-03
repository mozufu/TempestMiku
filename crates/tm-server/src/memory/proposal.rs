use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};
use crate::{Result, ServerError};

use super::util::{clean_required, encode_memory_segment, memory_dedupe_key, memory_record_id};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteKind {
    ProfileFact,
    RecallChunk,
}

impl MemoryWriteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProfileFact => "profile_fact",
            Self::RecallChunk => "recall_chunk",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteStatus {
    Pending,
    Approved,
    Denied,
    TimedOut,
    Cancelled,
}

impl MemoryWriteStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWriteProposal {
    pub proposal_id: Uuid,
    pub memory_kind: MemoryWriteKind,
    pub subject: String,
    pub scope: String,
    pub text: String,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub confidence: Option<f32>,
    pub source: String,
    pub provenance_label: String,
    pub provenance: Value,
    pub dedupe_key: String,
    pub record_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordRef {
    pub id: Uuid,
    pub uri: String,
    pub kind: MemoryWriteKind,
}

impl MemoryWriteProposal {
    pub fn profile_fact(
        subject: String,
        predicate: String,
        object: String,
        confidence: f32,
        source: String,
        provenance_label: String,
        provenance: Value,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        let subject = clean_required("subject", subject)?;
        let predicate = clean_required("predicate", predicate)?;
        let object = clean_required("object", object)?;
        let text = format!("{subject} {predicate} {object}");
        let dedupe_key = memory_dedupe_key(&["profile_fact", &subject, &predicate, &object]);
        Ok(Self {
            proposal_id: Uuid::new_v4(),
            memory_kind: MemoryWriteKind::ProfileFact,
            subject,
            scope: "global".to_string(),
            text,
            predicate: Some(predicate),
            object: Some(object),
            confidence: Some(confidence.clamp(0.0, 1.0)),
            source,
            provenance_label,
            provenance,
            record_id: memory_record_id("profile_fact", &dedupe_key),
            dedupe_key,
            created_at,
        })
    }

    pub fn recall_chunk(
        subject: String,
        scope: String,
        text: String,
        source: String,
        provenance_label: String,
        provenance: Value,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        let subject = clean_required("subject", subject)?;
        let scope = clean_required("scope", scope)?;
        let text = clean_required("text", text)?;
        let dedupe_key = memory_dedupe_key(&["recall_chunk", &scope, &text]);
        Ok(Self {
            proposal_id: Uuid::new_v4(),
            memory_kind: MemoryWriteKind::RecallChunk,
            subject,
            scope,
            text,
            predicate: None,
            object: None,
            confidence: None,
            source,
            provenance_label,
            provenance,
            record_id: memory_record_id("recall_chunk", &dedupe_key),
            dedupe_key,
            created_at,
        })
    }

    pub fn event_payload(
        &self,
        status: MemoryWriteStatus,
        record: Option<&MemoryRecordRef>,
    ) -> Value {
        json!({
            "kind": "memory",
            "proposalId": self.proposal_id,
            "memoryKind": self.memory_kind,
            "status": status,
            "subject": self.subject,
            "scope": self.scope,
            "text": self.text,
            "predicate": self.predicate,
            "object": self.object,
            "confidence": self.confidence,
            "source": self.source,
            "provenanceLabel": self.provenance_label,
            "provenance": self.provenance,
            "dedupeKey": self.dedupe_key,
            "recordId": self.record_id,
            "record": record,
            "createdAt": self.created_at,
        })
    }

    pub fn approval_scope(&self) -> Value {
        json!({
            "kind": "memory",
            "proposalId": self.proposal_id,
            "memoryKind": self.memory_kind,
            "subject": self.subject,
            "scope": self.scope,
            "text": self.text,
            "predicate": self.predicate,
            "object": self.object,
            "confidence": self.confidence,
            "provenanceLabel": self.provenance_label,
            "dedupeKey": self.dedupe_key,
            "recordId": self.record_id,
        })
    }

    pub fn record_ref(&self) -> MemoryRecordRef {
        MemoryRecordRef {
            id: self.record_id,
            uri: match self.memory_kind {
                MemoryWriteKind::ProfileFact => {
                    format!(
                        "memory://profile/{}/facts/{}",
                        encode_memory_segment(&self.subject),
                        self.record_id
                    )
                }
                MemoryWriteKind::RecallChunk => {
                    format!(
                        "memory://scopes/{}/chunks/{}",
                        encode_memory_segment(&self.scope),
                        self.record_id
                    )
                }
            },
            kind: self.memory_kind,
        }
    }
}

pub fn profile_fact_record(proposal: &MemoryWriteProposal) -> Result<ProfileFactRecord> {
    if proposal.memory_kind != MemoryWriteKind::ProfileFact {
        return Err(ServerError::InvalidRequest(
            "memory proposal is not a profile fact".to_string(),
        ));
    }
    Ok(ProfileFactRecord {
        id: proposal.record_id,
        subject: proposal.subject.clone(),
        predicate: proposal.predicate.clone().ok_or_else(|| {
            ServerError::InvalidRequest("profile fact proposal is missing predicate".to_string())
        })?,
        object: proposal.object.clone().ok_or_else(|| {
            ServerError::InvalidRequest("profile fact proposal is missing object".to_string())
        })?,
        confidence: proposal.confidence.unwrap_or(0.8),
        provenance: proposal.provenance_label.clone(),
        valid_from: proposal.created_at,
        valid_to: None,
    })
}

pub fn recall_chunk_record(proposal: &MemoryWriteProposal) -> Result<RecallChunkRecord> {
    if proposal.memory_kind != MemoryWriteKind::RecallChunk {
        return Err(ServerError::InvalidRequest(
            "memory proposal is not a recall chunk".to_string(),
        ));
    }
    Ok(RecallChunkRecord {
        id: proposal.record_id,
        scope: proposal.scope.clone(),
        text: proposal.text.clone(),
        source: proposal.source.clone(),
        created_at: proposal.created_at,
    })
}
