use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{EmbeddingConfig, EmbeddingProvenance, MemoryRecordKind};

use super::{DurableMemoryRecordError, MAX_PGVECTOR_INDEX_DIMENSIONS};

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
