use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    EmbeddingConfig, EmbeddingProvenance, MemoryRecordContractError, MemoryRecordEvidence,
    MemoryRecordKind, MemoryRecordLinks, MemoryRecordResource,
};

/// The maximum vector dimensionality this project will index once P8.3 enables HNSW.
///
/// P8.2 does not create a vector index. Keeping the limit in the durable contract lets startup
/// state fail visibly instead of discovering an unusable dimensionality after records are queued.
pub const MAX_PGVECTOR_INDEX_DIMENSIONS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoredMemoryRecord {
    pub resource: MemoryRecordResource,
    pub content_key: String,
    pub version_key: String,
}

impl StoredMemoryRecord {
    pub fn new(mut resource: MemoryRecordResource) -> Result<Self, DurableMemoryRecordError> {
        canonicalize_postgres_timestamp_precision(&mut resource);
        resource.validate()?;
        let content = memory_content_key_material(&resource)?;
        let content_digest = format!("{:x}", Sha256::digest(content));
        let version = memory_version_key_material(&resource)?;
        let version_digest = format!("{:x}", Sha256::digest(version));
        let kind = resource.kind();
        let id = resource.id();
        Ok(Self {
            resource,
            // Content identity deliberately excludes the generated record UUID and mutable
            // lifecycle metadata. Retries which independently allocate an id therefore dedupe
            // at the same owner/scope, while a changed observation remains a new version.
            content_key: format!("memory-content-v1-{content_digest}"),
            version_key: format!("memory-record-v1:{}:{id}:{version_digest}", kind.as_str()),
        })
    }

    pub fn validate(&self) -> Result<(), DurableMemoryRecordError> {
        self.resource.validate()?;
        if self.content_key.trim().is_empty() {
            return Err(DurableMemoryRecordError::MissingKey("content_key"));
        }
        if self.version_key.trim().is_empty() {
            return Err(DurableMemoryRecordError::MissingKey("version_key"));
        }
        if self.is_legacy_mirror_key() {
            return Ok(());
        }
        let expected = Self::new(self.resource.clone())?;
        if self.content_key != expected.content_key {
            return Err(DurableMemoryRecordError::KeyMismatch("content_key"));
        }
        if self.version_key != expected.version_key {
            return Err(DurableMemoryRecordError::KeyMismatch("version_key"));
        }
        Ok(())
    }

    pub const fn kind(&self) -> MemoryRecordKind {
        self.resource.kind()
    }

    pub const fn id(&self) -> Uuid {
        self.resource.id()
    }

    fn is_legacy_mirror_key(&self) -> bool {
        let Some(content_digest) = self.content_key.strip_prefix("legacy-memory-content-v1:")
        else {
            return false;
        };
        let version_prefix = format!(
            "legacy-memory-record-v1:{}:{}:",
            self.kind().as_str(),
            self.id()
        );
        self.version_key
            .strip_prefix(&version_prefix)
            .is_some_and(|version_digest| {
                version_digest == content_digest
                    && content_digest.len() == 32
                    && content_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    }
}

fn canonicalize_postgres_timestamp_precision(resource: &mut MemoryRecordResource) {
    // PostgreSQL `timestamptz` stores microseconds, while Linux clocks can supply nanoseconds.
    // Hash the same canonical values that the durable store will return on retry and replay.
    fn to_microseconds(value: DateTime<Utc>) -> DateTime<Utc> {
        DateTime::from_timestamp_micros(value.timestamp_micros())
            .expect("an existing DateTime remains representable at microsecond precision")
    }

    let (observed_at, effective_from, effective_to, created_at) = match resource {
        MemoryRecordResource::Episodic(record) => (
            &mut record.observed_at,
            &mut record.effective_from,
            &mut record.effective_to,
            &mut record.created_at,
        ),
        MemoryRecordResource::Semantic(record) => (
            &mut record.observed_at,
            &mut record.effective_from,
            &mut record.effective_to,
            &mut record.created_at,
        ),
    };
    *observed_at = to_microseconds(*observed_at);
    *effective_from = to_microseconds(*effective_from);
    *effective_to = effective_to.map(to_microseconds);
    *created_at = to_microseconds(*created_at);
}

#[derive(Serialize)]
#[serde(tag = "recordType", rename_all = "snake_case")]
enum MemoryContentKeyMaterial<'a> {
    Episodic {
        schema_version: u16,
        owner_subject: &'a str,
        memory_scope: &'a str,
        text: &'a str,
    },
    Semantic {
        schema_version: u16,
        owner_subject: &'a str,
        memory_scope: &'a str,
        semantic_subject: &'a str,
        predicate: &'a str,
        object: &'a str,
    },
}

#[derive(Serialize)]
#[serde(tag = "recordType", rename_all = "snake_case")]
enum MemoryVersionKeyMaterial<'a> {
    Episodic {
        schema_version: u16,
        id: Uuid,
        owner_subject: &'a str,
        memory_scope: &'a str,
        text: &'a str,
        evidence: &'a [MemoryRecordEvidence],
        confidence: f32,
        importance: f32,
        observed_at: DateTime<Utc>,
        effective_from: DateTime<Utc>,
        effective_to: Option<DateTime<Utc>>,
        status: crate::MemoryRecordStatus,
        links: &'a MemoryRecordLinks,
    },
    Semantic {
        schema_version: u16,
        id: Uuid,
        owner_subject: &'a str,
        memory_scope: &'a str,
        semantic_subject: &'a str,
        predicate: &'a str,
        object: &'a str,
        evidence: &'a [MemoryRecordEvidence],
        confidence: f32,
        importance: f32,
        observed_at: DateTime<Utc>,
        effective_from: DateTime<Utc>,
        effective_to: Option<DateTime<Utc>>,
        status: crate::MemoryRecordStatus,
        links: &'a MemoryRecordLinks,
    },
}

fn memory_content_key_material(
    resource: &MemoryRecordResource,
) -> Result<Vec<u8>, DurableMemoryRecordError> {
    let material = match resource {
        MemoryRecordResource::Episodic(record) => MemoryContentKeyMaterial::Episodic {
            schema_version: record.schema_version,
            owner_subject: &record.owner_subject,
            memory_scope: &record.memory_scope,
            text: &record.text,
        },
        MemoryRecordResource::Semantic(record) => MemoryContentKeyMaterial::Semantic {
            schema_version: record.schema_version,
            owner_subject: &record.owner_subject,
            memory_scope: &record.memory_scope,
            semantic_subject: &record.semantic_subject,
            predicate: &record.predicate,
            object: &record.object,
        },
    };
    serde_json::to_vec(&material)
        .map_err(|error| DurableMemoryRecordError::Serialization(error.to_string()))
}

fn memory_version_key_material(
    resource: &MemoryRecordResource,
) -> Result<Vec<u8>, DurableMemoryRecordError> {
    // `created_at` remains immutable in Postgres across an idempotent update, so it is not part
    // of a version key. Every field that describes the current record state remains covered.
    let material = match resource {
        MemoryRecordResource::Episodic(record) => MemoryVersionKeyMaterial::Episodic {
            schema_version: record.schema_version,
            id: record.id,
            owner_subject: &record.owner_subject,
            memory_scope: &record.memory_scope,
            text: &record.text,
            evidence: &record.evidence,
            confidence: record.confidence,
            importance: record.importance,
            observed_at: record.observed_at,
            effective_from: record.effective_from,
            effective_to: record.effective_to,
            status: record.status,
            links: &record.links,
        },
        MemoryRecordResource::Semantic(record) => MemoryVersionKeyMaterial::Semantic {
            schema_version: record.schema_version,
            id: record.id,
            owner_subject: &record.owner_subject,
            memory_scope: &record.memory_scope,
            semantic_subject: &record.semantic_subject,
            predicate: &record.predicate,
            object: &record.object,
            evidence: &record.evidence,
            confidence: record.confidence,
            importance: record.importance,
            observed_at: record.observed_at,
            effective_from: record.effective_from,
            effective_to: record.effective_to,
            status: record.status,
            links: &record.links,
        },
    };
    serde_json::to_vec(&material)
        .map_err(|error| DurableMemoryRecordError::Serialization(error.to_string()))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DurableMemoryRecordError {
    #[error(transparent)]
    Contract(#[from] MemoryRecordContractError),
    #[error("could not serialize memory record for a durable content key: {0}")]
    Serialization(String),
    #[error("durable memory {0} must not be empty")]
    MissingKey(&'static str),
    #[error("durable memory {0} does not match the record it identifies")]
    KeyMismatch(&'static str),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEmbeddingJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl MemoryEmbeddingJobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewMemoryEmbeddingJob {
    pub id: Uuid,
    pub record_kind: MemoryRecordKind,
    pub record_id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub content_key: String,
    pub provenance: EmbeddingProvenance,
    pub input_limit_bytes: usize,
    pub created_at: DateTime<Utc>,
}

impl NewMemoryEmbeddingJob {
    pub fn reembedding_key(&self) -> String {
        self.provenance.reembedding_key(self.record_id)
    }

    pub fn validate(&self) -> Result<(), DurableMemoryRecordError> {
        if self.owner_subject.trim().is_empty() {
            return Err(DurableMemoryRecordError::MissingKey("owner_subject"));
        }
        if self.memory_scope != "global"
            && self
                .memory_scope
                .strip_prefix("project:")
                .is_none_or(|slug| slug.trim().is_empty())
        {
            return Err(DurableMemoryRecordError::MissingKey("memory_scope"));
        }
        if self.content_key.trim().is_empty() {
            return Err(DurableMemoryRecordError::MissingKey("content_key"));
        }
        if self.input_limit_bytes == 0 || self.input_limit_bytes > crate::MAX_EMBEDDING_INPUT_BYTES
        {
            return Err(DurableMemoryRecordError::MissingKey("input_limit_bytes"));
        }
        self.provenance
            .validate()
            .map_err(|error| DurableMemoryRecordError::Serialization(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEmbeddingJobRecord {
    pub id: Uuid,
    pub record_kind: MemoryRecordKind,
    pub record_id: Uuid,
    pub owner_subject: String,
    pub memory_scope: String,
    pub content_key: String,
    pub provenance: EmbeddingProvenance,
    pub reembedding_key: String,
    pub status: MemoryEmbeddingJobStatus,
    pub input_limit_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    pub attempts: i32,
    pub available_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub lease_owner: Option<Uuid>,
    pub lease_epoch: i32,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEmbeddingGenerationStatus {
    Staging,
    Ready,
    Failed,
}

impl MemoryEmbeddingGenerationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "staging" => Some(Self::Staging),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewMemoryEmbeddingGeneration {
    pub owner_subject: String,
    pub memory_scope: String,
    pub embedding_version: String,
    pub provider: crate::EmbeddingProvider,
    pub model_id: String,
    pub dimensions: usize,
    pub normalization: crate::EmbeddingNormalization,
    pub input_limit_bytes: usize,
    pub created_at: DateTime<Utc>,
}

impl NewMemoryEmbeddingGeneration {
    pub fn from_config(
        owner_subject: impl Into<String>,
        memory_scope: impl Into<String>,
        config: &EmbeddingConfig,
        created_at: DateTime<Utc>,
    ) -> Result<Self, DurableMemoryRecordError> {
        config
            .validate()
            .map_err(|error| DurableMemoryRecordError::Serialization(error.to_string()))?;
        if !config.is_enabled() {
            return Err(DurableMemoryRecordError::Serialization(
                "disabled embeddings cannot create a generation".to_string(),
            ));
        }
        let model_id = config
            .model
            .clone()
            .expect("validated enabled config has a model");
        let dimensions = config
            .dimensions
            .expect("validated enabled config has dimensions");
        if dimensions > MAX_PGVECTOR_INDEX_DIMENSIONS {
            return Err(DurableMemoryRecordError::Serialization(format!(
                "embedding dimensions {dimensions} exceed the pgvector index bound"
            )));
        }
        Ok(Self {
            owner_subject: owner_subject.into(),
            memory_scope: memory_scope.into(),
            embedding_version: EmbeddingProvenance::version_id(
                config.provider,
                &model_id,
                dimensions,
                config.normalization,
            ),
            provider: config.provider,
            model_id,
            dimensions,
            normalization: config.normalization,
            input_limit_bytes: config.max_input_bytes,
            created_at,
        })
    }

    pub fn validate(&self) -> Result<(), DurableMemoryRecordError> {
        if self.owner_subject.trim().is_empty() {
            return Err(DurableMemoryRecordError::MissingKey("owner_subject"));
        }
        if self.memory_scope != "global"
            && self
                .memory_scope
                .strip_prefix("project:")
                .is_none_or(|slug| slug.trim().is_empty())
        {
            return Err(DurableMemoryRecordError::MissingKey("memory_scope"));
        }
        if self.provider == crate::EmbeddingProvider::Disabled {
            return Err(DurableMemoryRecordError::MissingKey("provider"));
        }
        if self.model_id.trim().is_empty() {
            return Err(DurableMemoryRecordError::MissingKey("model_id"));
        }
        if self.dimensions == 0 || self.dimensions > MAX_PGVECTOR_INDEX_DIMENSIONS {
            return Err(DurableMemoryRecordError::MissingKey("dimensions"));
        }
        if self.input_limit_bytes == 0 || self.input_limit_bytes > crate::MAX_EMBEDDING_INPUT_BYTES
        {
            return Err(DurableMemoryRecordError::MissingKey("input_limit_bytes"));
        }
        let expected = EmbeddingProvenance::version_id(
            self.provider,
            &self.model_id,
            self.dimensions,
            self.normalization,
        );
        if self.embedding_version != expected {
            return Err(DurableMemoryRecordError::KeyMismatch("embedding_version"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEmbeddingGeneration {
    pub owner_subject: String,
    pub memory_scope: String,
    pub embedding_version: String,
    pub provider: crate::EmbeddingProvider,
    pub model_id: String,
    pub dimensions: usize,
    pub normalization: crate::EmbeddingNormalization,
    pub generation_order: i64,
    pub snapshot_revision: i64,
    pub input_limit_bytes: usize,
    pub vector_table: String,
    pub expected_records: usize,
    pub completed_records: usize,
    pub status: MemoryEmbeddingGenerationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEmbeddingJobLease {
    pub job: MemoryEmbeddingJobRecord,
    pub owner_id: Uuid,
    pub epoch: i32,
}

#[derive(Debug, Clone)]
pub struct MemoryEmbeddingJobClaim {
    pub owner_subject: String,
    pub memory_scope: String,
    pub embedding_version: String,
    pub owner_id: Uuid,
    pub now: DateTime<Utc>,
    pub lease_timeout: chrono::Duration,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryScopeTombstone {
    pub owner_subject: String,
    pub memory_scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_alias: Option<String>,
    pub reason: String,
    pub revoked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySchemaReadiness {
    Ready,
    Corrupt { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PgVectorReadiness {
    Disabled,
    Ready,
    Missing,
    UnsupportedDimensions { dimensions: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingReadiness {
    Disabled,
    Ready,
    TemporarilyUnavailable { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DurableMemoryReadiness {
    pub schema: MemorySchemaReadiness,
    pub pgvector: PgVectorReadiness,
    pub embeddings: EmbeddingReadiness,
}

impl DurableMemoryReadiness {
    pub fn from_embedding_config(
        schema: MemorySchemaReadiness,
        embeddings: &EmbeddingConfig,
        pgvector_available: bool,
    ) -> Self {
        if !embeddings.is_enabled() {
            return Self {
                schema,
                pgvector: PgVectorReadiness::Disabled,
                embeddings: EmbeddingReadiness::Disabled,
            };
        }

        let dimensions = embeddings.dimensions.unwrap_or(0);
        let pgvector = if dimensions == 0 || dimensions > MAX_PGVECTOR_INDEX_DIMENSIONS {
            PgVectorReadiness::UnsupportedDimensions { dimensions }
        } else if pgvector_available {
            PgVectorReadiness::Ready
        } else {
            PgVectorReadiness::Missing
        };
        let embedding_readiness = match pgvector {
            PgVectorReadiness::Ready => EmbeddingReadiness::Ready,
            PgVectorReadiness::Missing => EmbeddingReadiness::TemporarilyUnavailable {
                reason: "pgvector is unavailable for the configured embedding provider".to_string(),
            },
            PgVectorReadiness::UnsupportedDimensions { .. } => {
                EmbeddingReadiness::TemporarilyUnavailable {
                    reason: "configured embedding dimensions cannot be indexed".to_string(),
                }
            }
            PgVectorReadiness::Disabled => EmbeddingReadiness::Disabled,
        };
        Self {
            schema,
            pgvector,
            embeddings: embedding_readiness,
        }
    }

    pub const fn allows_durable_writes(&self) -> bool {
        matches!(self.schema, MemorySchemaReadiness::Ready)
    }

    pub const fn dense_retrieval_ready(&self) -> bool {
        matches!(self.schema, MemorySchemaReadiness::Ready)
            && matches!(self.pgvector, PgVectorReadiness::Ready)
            && matches!(self.embeddings, EmbeddingReadiness::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EpisodicMemoryRecord, MEMORY_RECORD_SCHEMA_VERSION, MemoryEvidenceSource,
        MemoryRecordEvidence, MemoryRecordLinks, MemoryRecordStatus,
    };

    fn record() -> MemoryRecordResource {
        MemoryRecordResource::Episodic(EpisodicMemoryRecord {
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
            id: Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap(),
            owner_subject: "brian".to_string(),
            memory_scope: "global".to_string(),
            text: "The durable spine keeps a lexical fallback.".to_string(),
            evidence: vec![MemoryRecordEvidence {
                schema_version: MEMORY_RECORD_SCHEMA_VERSION,
                label: "fixture".to_string(),
                source: MemoryEvidenceSource::Resource {
                    uri: "memory://fixtures/durable".to_string(),
                },
            }],
            confidence: 0.9,
            importance: 0.8,
            observed_at: "2026-07-15T00:00:00Z".parse().unwrap(),
            effective_from: "2026-07-15T00:00:00Z".parse().unwrap(),
            effective_to: None,
            status: MemoryRecordStatus::Active,
            links: MemoryRecordLinks::default(),
            created_at: "2026-07-15T00:00:00Z".parse().unwrap(),
        })
    }

    #[test]
    fn stored_record_keys_are_deterministic_and_versioned() {
        let first = StoredMemoryRecord::new(record()).unwrap();
        let second = StoredMemoryRecord::new(record()).unwrap();
        assert_eq!(first, second);
        assert!(first.content_key.starts_with("memory-content-v1-"));
        assert!(first.version_key.contains(":episodic:"));
    }

    #[test]
    fn stored_record_keys_use_postgres_microsecond_timestamp_precision() {
        let mut resource = record();
        let MemoryRecordResource::Episodic(value) = &mut resource else {
            unreachable!("fixture is episodic");
        };
        value.observed_at = "2026-07-15T00:00:00.123456789Z".parse().unwrap();
        value.effective_from = "2026-07-15T00:00:00.234567891Z".parse().unwrap();
        value.effective_to = Some("2026-07-15T00:00:01.345678912Z".parse().unwrap());
        value.created_at = "2026-07-15T00:00:00.456789123Z".parse().unwrap();

        let stored = StoredMemoryRecord::new(resource).unwrap();
        let MemoryRecordResource::Episodic(value) = &stored.resource else {
            unreachable!("fixture is episodic");
        };
        assert_eq!(
            value.observed_at.to_rfc3339(),
            "2026-07-15T00:00:00.123456+00:00"
        );
        assert_eq!(
            value.effective_from.to_rfc3339(),
            "2026-07-15T00:00:00.234567+00:00"
        );
        assert_eq!(
            value.effective_to.unwrap().to_rfc3339(),
            "2026-07-15T00:00:01.345678+00:00"
        );
        assert_eq!(
            value.created_at.to_rfc3339(),
            "2026-07-15T00:00:00.456789+00:00"
        );
        stored.validate().unwrap();
    }

    #[test]
    fn content_key_deduplicates_independently_generated_record_ids() {
        let first = StoredMemoryRecord::new(record()).unwrap();
        let mut retried = record();
        let MemoryRecordResource::Episodic(value) = &mut retried else {
            unreachable!("fixture is episodic");
        };
        value.id = Uuid::new_v4();
        value.observed_at += chrono::Duration::seconds(1);
        let retried = StoredMemoryRecord::new(retried).unwrap();

        assert_eq!(first.content_key, retried.content_key);
        assert_ne!(first.version_key, retried.version_key);
    }

    #[test]
    fn persisted_keys_fail_closed_when_the_record_does_not_match() {
        let mut stored = StoredMemoryRecord::new(record()).unwrap();
        stored.content_key = "memory-content-v1-tampered".to_string();
        assert_eq!(
            stored.validate(),
            Err(DurableMemoryRecordError::KeyMismatch("content_key"))
        );
    }

    #[test]
    fn readiness_keeps_lexical_durable_writes_available_when_dense_is_disabled() {
        let readiness = DurableMemoryReadiness::from_embedding_config(
            MemorySchemaReadiness::Ready,
            &EmbeddingConfig::default(),
            false,
        );
        assert!(readiness.allows_durable_writes());
        assert!(!readiness.dense_retrieval_ready());
        assert_eq!(readiness.pgvector, PgVectorReadiness::Disabled);
        assert_eq!(readiness.embeddings, EmbeddingReadiness::Disabled);
    }

    #[test]
    fn readiness_distinguishes_missing_vector_and_unsupported_dimensions() {
        let missing = DurableMemoryReadiness::from_embedding_config(
            MemorySchemaReadiness::Ready,
            &EmbeddingConfig {
                provider: crate::EmbeddingProvider::Local,
                dimensions: Some(768),
                model: Some("local-test".to_string()),
                ..EmbeddingConfig::default()
            },
            false,
        );
        assert_eq!(missing.pgvector, PgVectorReadiness::Missing);

        let unsupported = DurableMemoryReadiness::from_embedding_config(
            MemorySchemaReadiness::Ready,
            &EmbeddingConfig {
                provider: crate::EmbeddingProvider::Local,
                dimensions: Some(MAX_PGVECTOR_INDEX_DIMENSIONS + 1),
                model: Some("local-test".to_string()),
                ..EmbeddingConfig::default()
            },
            true,
        );
        assert_eq!(
            unsupported.pgvector,
            PgVectorReadiness::UnsupportedDimensions {
                dimensions: MAX_PGVECTOR_INDEX_DIMENSIONS + 1
            }
        );
    }
}
