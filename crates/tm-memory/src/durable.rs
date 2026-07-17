use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    MemoryRecordContractError, MemoryRecordEvidence, MemoryRecordKind, MemoryRecordLinks,
    MemoryRecordResource,
};

mod embedding_jobs;
mod readiness;

pub use embedding_jobs::{
    MemoryEmbeddingGeneration, MemoryEmbeddingGenerationStatus, MemoryEmbeddingJobClaim,
    MemoryEmbeddingJobLease, MemoryEmbeddingJobRecord, MemoryEmbeddingJobStatus,
    MemoryScopeTombstone, NewMemoryEmbeddingGeneration, NewMemoryEmbeddingJob,
};
pub use readiness::{
    DurableMemoryReadiness, EmbeddingReadiness, MemorySchemaReadiness, PgVectorReadiness,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EmbeddingConfig, EpisodicMemoryRecord, MEMORY_RECORD_SCHEMA_VERSION, MemoryEvidenceSource,
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
