use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MEMORY_RECORD_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordStatus {
    Candidate,
    Active,
    Withheld,
    Unsupported,
    Corrected,
    Superseded,
}

impl MemoryRecordStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Withheld => "withheld",
            Self::Unsupported => "unsupported",
            Self::Corrected => "corrected",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "candidate" => Some(Self::Candidate),
            "active" => Some(Self::Active),
            "withheld" => Some(Self::Withheld),
            "unsupported" => Some(Self::Unsupported),
            "corrected" => Some(Self::Corrected),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }

    pub fn is_retrievable(self) -> bool {
        self == Self::Active
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordKind {
    Episodic,
    Semantic,
}

impl MemoryRecordKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "episodic" => Some(Self::Episodic),
            "semantic" => Some(Self::Semantic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordLinks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrects_record_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_by_record_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_record_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by_record_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum MemoryEvidenceSource {
    SessionEvent { session_id: Uuid, event_seq: i64 },
    SessionMessage { session_id: Uuid, message_seq: i64 },
    Resource { uri: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordEvidence {
    pub schema_version: u16,
    pub label: String,
    pub source: MemoryEvidenceSource,
}

impl MemoryRecordEvidence {
    pub fn session_event(session_id: Uuid, event_seq: i64, label: impl Into<String>) -> Self {
        Self {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            label: label.into(),
            source: MemoryEvidenceSource::SessionEvent {
                session_id,
                event_seq,
            },
        }
    }

    pub fn session_message(session_id: Uuid, message_seq: i64, label: impl Into<String>) -> Self {
        Self {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            label: label.into(),
            source: MemoryEvidenceSource::SessionMessage {
                session_id,
                message_seq,
            },
        }
    }

    pub fn resource(uri: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            label: label.into(),
            source: MemoryEvidenceSource::Resource { uri: uri.into() },
        }
    }

    pub fn validate(&self) -> Result<(), MemoryRecordContractError> {
        validate_schema_version(self.schema_version)?;
        if self.label.trim().is_empty() {
            return Err(MemoryRecordContractError::MissingField("evidence.label"));
        }
        match &self.source {
            MemoryEvidenceSource::SessionEvent { event_seq, .. } if *event_seq <= 0 => {
                Err(MemoryRecordContractError::InvalidSequence("event_seq"))
            }
            MemoryEvidenceSource::SessionMessage { message_seq, .. } if *message_seq <= 0 => {
                Err(MemoryRecordContractError::InvalidSequence("message_seq"))
            }
            MemoryEvidenceSource::Resource { uri }
                if uri.trim().is_empty() || !uri.contains("://") =>
            {
                Err(MemoryRecordContractError::InvalidResourceUri)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EpisodicMemoryRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub text: String,
    pub evidence: Vec<MemoryRecordEvidence>,
    pub confidence: f32,
    pub importance: f32,
    pub observed_at: DateTime<Utc>,
    pub effective_from: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_to: Option<DateTime<Utc>>,
    pub status: MemoryRecordStatus,
    #[serde(default)]
    pub links: MemoryRecordLinks,
    pub created_at: DateTime<Utc>,
}

impl EpisodicMemoryRecord {
    pub fn from_recall_chunk(
        owner_subject: impl Into<String>,
        chunk: RecallChunkRecord,
        evidence: Vec<MemoryRecordEvidence>,
    ) -> Result<Self, MemoryRecordContractError> {
        let record = Self {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id: chunk.id,
            owner_subject: owner_subject.into(),
            memory_scope: chunk.scope,
            text: chunk.text,
            evidence,
            confidence: 1.0,
            importance: chunk.importance,
            observed_at: chunk.created_at,
            effective_from: chunk.created_at,
            effective_to: None,
            status: MemoryRecordStatus::Active,
            links: MemoryRecordLinks::default(),
            created_at: chunk.created_at,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), MemoryRecordContractError> {
        validate_common_record(
            self.schema_version,
            self.id,
            &self.owner_subject,
            &self.memory_scope,
            &self.text,
            &self.evidence,
            self.confidence,
            self.importance,
            self.effective_from,
            self.effective_to,
            &self.links,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMemoryRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub semantic_subject: String,
    pub predicate: String,
    pub object: String,
    pub evidence: Vec<MemoryRecordEvidence>,
    pub confidence: f32,
    pub importance: f32,
    pub observed_at: DateTime<Utc>,
    pub effective_from: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_to: Option<DateTime<Utc>>,
    pub status: MemoryRecordStatus,
    #[serde(default)]
    pub links: MemoryRecordLinks,
    pub created_at: DateTime<Utc>,
}

impl SemanticMemoryRecord {
    pub fn from_profile_fact(
        owner_subject: impl Into<String>,
        memory_scope: impl Into<String>,
        fact: ProfileFactRecord,
        evidence: Vec<MemoryRecordEvidence>,
    ) -> Result<Self, MemoryRecordContractError> {
        let status = if fact.valid_to.is_some() {
            MemoryRecordStatus::Superseded
        } else {
            MemoryRecordStatus::Active
        };
        let record = Self {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id: fact.id,
            owner_subject: owner_subject.into(),
            memory_scope: memory_scope.into(),
            semantic_subject: fact.subject,
            predicate: fact.predicate,
            object: fact.object,
            evidence,
            confidence: fact.confidence,
            importance: fact.importance,
            observed_at: fact.valid_from,
            effective_from: fact.valid_from,
            effective_to: fact.valid_to,
            status,
            links: MemoryRecordLinks::default(),
            created_at: fact.valid_from,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), MemoryRecordContractError> {
        for (field, value) in [
            ("semantic_subject", self.semantic_subject.as_str()),
            ("predicate", self.predicate.as_str()),
            ("object", self.object.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(MemoryRecordContractError::MissingField(field));
            }
        }
        validate_common_record(
            self.schema_version,
            self.id,
            &self.owner_subject,
            &self.memory_scope,
            &self.object,
            &self.evidence,
            self.confidence,
            self.importance,
            self.effective_from,
            self.effective_to,
            &self.links,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "recordType", content = "record", rename_all = "snake_case")]
pub enum MemoryRecordResource {
    Episodic(EpisodicMemoryRecord),
    Semantic(SemanticMemoryRecord),
}

impl MemoryRecordResource {
    pub const fn kind(&self) -> MemoryRecordKind {
        match self {
            Self::Episodic(_) => MemoryRecordKind::Episodic,
            Self::Semantic(_) => MemoryRecordKind::Semantic,
        }
    }

    pub const fn id(&self) -> Uuid {
        match self {
            Self::Episodic(record) => record.id,
            Self::Semantic(record) => record.id,
        }
    }

    pub fn owner_subject(&self) -> &str {
        match self {
            Self::Episodic(record) => &record.owner_subject,
            Self::Semantic(record) => &record.owner_subject,
        }
    }

    pub fn memory_scope(&self) -> &str {
        match self {
            Self::Episodic(record) => &record.memory_scope,
            Self::Semantic(record) => &record.memory_scope,
        }
    }

    pub const fn status(&self) -> MemoryRecordStatus {
        match self {
            Self::Episodic(record) => record.status,
            Self::Semantic(record) => record.status,
        }
    }

    pub const fn effective_to(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Episodic(record) => record.effective_to,
            Self::Semantic(record) => record.effective_to,
        }
    }

    pub const fn importance(&self) -> f32 {
        match self {
            Self::Episodic(record) => record.importance,
            Self::Semantic(record) => record.importance,
        }
    }

    pub const fn observed_at(&self) -> DateTime<Utc> {
        match self {
            Self::Episodic(record) => record.observed_at,
            Self::Semantic(record) => record.observed_at,
        }
    }

    pub fn links(&self) -> &MemoryRecordLinks {
        match self {
            Self::Episodic(record) => &record.links,
            Self::Semantic(record) => &record.links,
        }
    }

    pub fn evidence(&self) -> &[MemoryRecordEvidence] {
        match self {
            Self::Episodic(record) => &record.evidence,
            Self::Semantic(record) => &record.evidence,
        }
    }

    pub fn validate(&self) -> Result<(), MemoryRecordContractError> {
        match self {
            Self::Episodic(record) => record.validate(),
            Self::Semantic(record) => record.validate(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryRecordContractError {
    #[error("unsupported memory record schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("memory record field {0} must not be empty")]
    MissingField(&'static str),
    #[error("memory scope must be global or project:<slug>")]
    InvalidMemoryScope,
    #[error("memory record must cite at least one evidence source")]
    MissingEvidence,
    #[error("memory record {0} must be finite and within 0..=1")]
    InvalidScore(&'static str),
    #[error("memory record effective_to precedes effective_from")]
    InvalidEffectiveRange,
    #[error("memory evidence {0} must be positive")]
    InvalidSequence(&'static str),
    #[error("memory evidence resource URI must be an absolute resource URI")]
    InvalidResourceUri,
    #[error("memory record correction/supersession links must not self-reference")]
    SelfReferentialLink,
}

fn validate_schema_version(schema_version: u16) -> Result<(), MemoryRecordContractError> {
    if schema_version == MEMORY_RECORD_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(MemoryRecordContractError::UnsupportedSchemaVersion(
            schema_version,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_common_record(
    schema_version: u16,
    record_id: Uuid,
    owner_subject: &str,
    memory_scope: &str,
    content: &str,
    evidence: &[MemoryRecordEvidence],
    confidence: f32,
    importance: f32,
    effective_from: DateTime<Utc>,
    effective_to: Option<DateTime<Utc>>,
    links: &MemoryRecordLinks,
) -> Result<(), MemoryRecordContractError> {
    validate_schema_version(schema_version)?;
    if owner_subject.trim().is_empty() {
        return Err(MemoryRecordContractError::MissingField("owner_subject"));
    }
    if memory_scope != "global"
        && memory_scope
            .strip_prefix("project:")
            .is_none_or(|slug| slug.trim().is_empty())
    {
        return Err(MemoryRecordContractError::InvalidMemoryScope);
    }
    if content.trim().is_empty() {
        return Err(MemoryRecordContractError::MissingField("content"));
    }
    if evidence.is_empty() {
        return Err(MemoryRecordContractError::MissingEvidence);
    }
    for item in evidence {
        item.validate()?;
    }
    validate_score("confidence", confidence)?;
    validate_score("importance", importance)?;
    if effective_to.is_some_and(|effective_to| effective_to < effective_from) {
        return Err(MemoryRecordContractError::InvalidEffectiveRange);
    }
    let ids = [
        links.corrects_record_id,
        links.corrected_by_record_id,
        links.supersedes_record_id,
        links.superseded_by_record_id,
    ];
    if ids
        .iter()
        .flatten()
        .any(|linked_id| linked_id.is_nil() || *linked_id == record_id)
    {
        return Err(MemoryRecordContractError::SelfReferentialLink);
    }
    Ok(())
}

fn validate_score(field: &'static str, score: f32) -> Result<(), MemoryRecordContractError> {
    if score.is_finite() && (0.0..=1.0).contains(&score) {
        Ok(())
    } else {
        Err(MemoryRecordContractError::InvalidScore(field))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileFactRecord {
    pub id: Uuid,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
    pub importance: f32,
    pub provenance: String,
    pub valid_from: DateTime<Utc>,
    pub valid_to: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecallChunkRecord {
    pub id: Uuid,
    pub scope: String,
    pub text: String,
    pub source: String,
    pub importance: f32,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn timestamp() -> DateTime<Utc> {
        "2026-07-15T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn versioned_records_serialize_authority_evidence_and_history_links() {
        let source_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
        let record_id = Uuid::parse_str("20000000-0000-0000-0000-000000000001").unwrap();
        let record = EpisodicMemoryRecord {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id: record_id,
            owner_subject: "brian".to_string(),
            memory_scope: "project:tempestmiku".to_string(),
            text: "P8 keeps correction evidence.".to_string(),
            evidence: vec![
                MemoryRecordEvidence::session_event(source_id, 7, "source event"),
                MemoryRecordEvidence::resource("artifact://p8/report", "report"),
            ],
            confidence: 0.9,
            importance: 0.8,
            observed_at: timestamp(),
            effective_from: timestamp(),
            effective_to: None,
            status: MemoryRecordStatus::Active,
            links: MemoryRecordLinks {
                corrects_record_id: Some(
                    Uuid::parse_str("20000000-0000-0000-0000-000000000000").unwrap(),
                ),
                ..MemoryRecordLinks::default()
            },
            created_at: timestamp(),
        };

        record.validate().unwrap();
        let serialized = serde_json::to_value(MemoryRecordResource::Episodic(record)).unwrap();
        assert_eq!(serialized["recordType"], json!("episodic"));
        assert_eq!(serialized["record"]["schemaVersion"], json!(1));
        assert_eq!(serialized["record"]["ownerSubject"], json!("brian"));
        assert_eq!(
            serialized["record"]["memoryScope"],
            json!("project:tempestmiku")
        );
        assert_eq!(
            serialized["record"]["evidence"][0]["source"]["kind"],
            json!("session_event")
        );
        assert_eq!(
            serialized["record"]["evidence"][0]["source"]["sessionId"],
            json!(source_id)
        );
        assert_eq!(
            serialized["record"]["evidence"][0]["source"]["eventSeq"],
            json!(7)
        );
        assert_eq!(
            serialized["record"]["evidence"][1]["source"]["uri"],
            json!("artifact://p8/report")
        );
        assert_eq!(
            serialized["record"]["links"]["correctsRecordId"],
            json!("20000000-0000-0000-0000-000000000000")
        );
    }

    #[test]
    fn record_contract_fails_closed_on_missing_authority_evidence_and_bad_scores() {
        let chunk = RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "session:ambient".to_string(),
            text: "must not persist".to_string(),
            source: "test".to_string(),
            importance: 2.0,
            created_at: timestamp(),
        };
        assert_eq!(
            EpisodicMemoryRecord::from_recall_chunk("", chunk, Vec::new()),
            Err(MemoryRecordContractError::MissingField("owner_subject"))
        );

        assert_eq!(
            MemoryRecordEvidence::resource("relative/path", "bad").validate(),
            Err(MemoryRecordContractError::InvalidResourceUri)
        );

        let record_id = Uuid::new_v4();
        let self_linked = EpisodicMemoryRecord {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id: record_id,
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            text: "self-linked history must fail".to_string(),
            evidence: vec![MemoryRecordEvidence::resource(
                "memory://fixtures/source",
                "source",
            )],
            confidence: 1.0,
            importance: 1.0,
            observed_at: timestamp(),
            effective_from: timestamp(),
            effective_to: None,
            status: MemoryRecordStatus::Active,
            links: MemoryRecordLinks {
                supersedes_record_id: Some(record_id),
                ..MemoryRecordLinks::default()
            },
            created_at: timestamp(),
        };
        assert_eq!(
            self_linked.validate(),
            Err(MemoryRecordContractError::SelfReferentialLink)
        );
    }

    #[test]
    fn legacy_records_map_without_inventing_authority() {
        let now = timestamp();
        let chunk = RecallChunkRecord {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            text: "Keep the lexical path available.".to_string(),
            source: "session:test".to_string(),
            importance: 0.75,
            created_at: now,
        };
        let episodic = EpisodicMemoryRecord::from_recall_chunk(
            "brian",
            chunk,
            vec![MemoryRecordEvidence::resource(
                "memory://scopes/global/chunks/source",
                "legacy recall source",
            )],
        )
        .unwrap();
        assert_eq!(episodic.owner_subject, "brian");
        assert_eq!(episodic.memory_scope, "global");
        assert_eq!(episodic.status, MemoryRecordStatus::Active);

        let fact = ProfileFactRecord {
            id: Uuid::new_v4(),
            subject: "brian".to_string(),
            predicate: "prefers".to_string(),
            object: "replayable evidence".to_string(),
            confidence: 0.9,
            importance: 0.8,
            provenance: "session:test".to_string(),
            valid_from: now,
            valid_to: Some(now),
        };
        let semantic = SemanticMemoryRecord::from_profile_fact(
            "brian",
            "global",
            fact,
            vec![MemoryRecordEvidence::session_message(
                Uuid::new_v4(),
                1,
                "legacy fact source",
            )],
        )
        .unwrap();
        assert_eq!(semantic.status, MemoryRecordStatus::Superseded);
        assert_eq!(semantic.effective_to, Some(now));
    }
}
