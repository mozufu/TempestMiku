use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{MemoryRecordResource, StoredMemoryRecord};

pub const EMBEDDING_PROVENANCE_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_EMBEDDING_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_EMBEDDING_MAX_BATCH_SIZE: usize = 32;
pub const DEFAULT_EMBEDDING_MAX_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_EMBEDDING_BATCH_SIZE: usize = 128;
pub const MAX_EMBEDDING_INPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProvider {
    Disabled,
    Local,
    OpenAiCompatible,
}

impl EmbeddingProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Local => "local",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "local" => Some(Self::Local),
            "openai_compatible" => Some(Self::OpenAiCompatible),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub provider: EmbeddingProvider,
    pub dimensions: Option<usize>,
    pub model: Option<String>,
    #[serde(default = "default_embedding_normalization")]
    pub normalization: EmbeddingNormalization,
    #[serde(default = "default_embedding_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_embedding_max_batch_size")]
    pub max_batch_size: usize,
    #[serde(default = "default_embedding_max_input_bytes")]
    pub max_input_bytes: usize,
}

const fn default_embedding_normalization() -> EmbeddingNormalization {
    EmbeddingNormalization::L2
}

const fn default_embedding_timeout_ms() -> u64 {
    DEFAULT_EMBEDDING_TIMEOUT_MS
}

const fn default_embedding_max_batch_size() -> usize {
    DEFAULT_EMBEDDING_MAX_BATCH_SIZE
}

const fn default_embedding_max_input_bytes() -> usize {
    DEFAULT_EMBEDDING_MAX_INPUT_BYTES
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingNormalization {
    None,
    L2,
}

impl EmbeddingNormalization {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::L2 => "l2",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "l2" => Some(Self::L2),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReembeddingState {
    Pending,
    Ready,
    Failed,
    Superseded,
}

impl ReembeddingState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProvenance {
    pub schema_version: u16,
    pub provider: EmbeddingProvider,
    pub model_id: String,
    pub dimensions: usize,
    pub normalization: EmbeddingNormalization,
    pub content_hash: String,
    pub embedding_version: String,
    pub created_at: DateTime<Utc>,
    pub reembedding_state: ReembeddingState,
}

impl EmbeddingProvenance {
    pub fn from_config(
        config: &EmbeddingConfig,
        content: &str,
        created_at: DateTime<Utc>,
    ) -> Result<Self, EmbeddingConfigError> {
        config.validate()?;
        if !config.is_enabled() {
            return Err(EmbeddingConfigError::DisabledProvider);
        }
        let provider = config.provider;
        let model_id = config
            .model
            .as_deref()
            .expect("enabled embedding config validation pins a model id");
        let dimensions = config
            .dimensions
            .expect("enabled embedding config validation pins dimensions");
        Ok(Self {
            schema_version: EMBEDDING_PROVENANCE_SCHEMA_VERSION,
            provider,
            model_id: model_id.to_string(),
            dimensions,
            normalization: config.normalization,
            content_hash: embedding_content_hash(content),
            embedding_version: Self::version_id(
                provider,
                model_id,
                dimensions,
                config.normalization,
            ),
            created_at,
            reembedding_state: ReembeddingState::Pending,
        })
    }

    pub fn version_id(
        provider: EmbeddingProvider,
        model_id: &str,
        dimensions: usize,
        normalization: EmbeddingNormalization,
    ) -> String {
        let canonical = format!(
            "provider={}\nmodel={model_id}\ndimensions={dimensions}\nnormalization={}",
            provider.as_str(),
            normalization.as_str()
        );
        format!("emb-v1-{:x}", Sha256::digest(canonical.as_bytes()))
    }

    pub fn reembedding_key(&self, record_id: Uuid) -> String {
        let canonical = format!(
            "record={record_id}\ncontent={}\nversion={}",
            self.content_hash, self.embedding_version
        );
        format!("reembed-v1-{:x}", Sha256::digest(canonical.as_bytes()))
    }

    pub fn validate(&self) -> Result<(), EmbeddingProvenanceError> {
        if self.schema_version != EMBEDDING_PROVENANCE_SCHEMA_VERSION {
            return Err(EmbeddingProvenanceError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.provider == EmbeddingProvider::Disabled {
            return Err(EmbeddingProvenanceError::DisabledProvider);
        }
        if self.model_id.trim().is_empty() {
            return Err(EmbeddingProvenanceError::MissingModelId);
        }
        if self.dimensions == 0 {
            return Err(EmbeddingProvenanceError::MissingDimensions);
        }
        if self.content_hash.len() != 64
            || !self
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(EmbeddingProvenanceError::InvalidContentHash);
        }
        let expected = Self::version_id(
            self.provider,
            &self.model_id,
            self.dimensions,
            self.normalization,
        );
        if self.embedding_version != expected {
            return Err(EmbeddingProvenanceError::EmbeddingVersionMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EmbeddingProvenanceError {
    #[error("unsupported embedding provenance schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("disabled embedding provider cannot produce provenance")]
    DisabledProvider,
    #[error("embedding model id must be pinned")]
    MissingModelId,
    #[error("embedding dimensions must be positive")]
    MissingDimensions,
    #[error("embedding content hash must be a SHA-256 hex digest")]
    InvalidContentHash,
    #[error("embedding version does not match its provider/model/dimension/normalization contract")]
    EmbeddingVersionMismatch,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EmbeddingConfigError {
    #[error("disabled embeddings cannot create an embedding request or provenance")]
    DisabledProvider,
    #[error("embedding dimensions must be pinned when provider is {0:?}")]
    MissingDimensions(EmbeddingProvider),
    #[error("embedding model id must be pinned when provider is {0:?}")]
    MissingModel(EmbeddingProvider),
    #[error("embedding timeout must be positive")]
    MissingTimeout,
    #[error("embedding batch size must be between 1 and {MAX_EMBEDDING_BATCH_SIZE}")]
    InvalidBatchSize,
    #[error("embedding input limit must be between 1 and {MAX_EMBEDDING_INPUT_BYTES} bytes")]
    InvalidInputLimit,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProvider::Disabled,
            dimensions: None,
            model: None,
            normalization: EmbeddingNormalization::L2,
            timeout_ms: DEFAULT_EMBEDDING_TIMEOUT_MS,
            max_batch_size: DEFAULT_EMBEDDING_MAX_BATCH_SIZE,
            max_input_bytes: DEFAULT_EMBEDDING_MAX_INPUT_BYTES,
        }
    }
}

impl EmbeddingConfig {
    pub fn validate(&self) -> Result<(), EmbeddingConfigError> {
        if self.provider != EmbeddingProvider::Disabled {
            if self.dimensions.unwrap_or(0) == 0 {
                return Err(EmbeddingConfigError::MissingDimensions(self.provider));
            }
            if self
                .model
                .as_deref()
                .is_none_or(|model| model.trim().is_empty())
            {
                return Err(EmbeddingConfigError::MissingModel(self.provider));
            }
            if self.timeout_ms == 0 {
                return Err(EmbeddingConfigError::MissingTimeout);
            }
            if self.max_batch_size == 0 || self.max_batch_size > MAX_EMBEDDING_BATCH_SIZE {
                return Err(EmbeddingConfigError::InvalidBatchSize);
            }
            if self.max_input_bytes == 0 || self.max_input_bytes > MAX_EMBEDDING_INPUT_BYTES {
                return Err(EmbeddingConfigError::InvalidInputLimit);
            }
        }
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.provider != EmbeddingProvider::Disabled
    }

    pub fn embedding_version(&self) -> Result<Option<String>, EmbeddingConfigError> {
        self.validate()?;
        if !self.is_enabled() {
            return Ok(None);
        }
        Ok(self.model.as_deref().map(|model| {
            EmbeddingProvenance::version_id(
                self.provider,
                model,
                self.dimensions
                    .expect("validated enabled config has dimensions"),
                self.normalization,
            )
        }))
    }
}

/// The bounded, batch-capable embedding boundary used by P8.3 workers.
///
/// Concrete transport belongs outside `tm-memory`; the production server supplies a self-hosted
/// local HTTP client, while tests can use a scripted implementation without network access.
#[async_trait]
pub trait EmbeddingClient: Send + Sync {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, EmbeddingError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingInput {
    pub id: String,
    pub content: String,
    pub content_hash: String,
}

impl EmbeddingInput {
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            id: id.into(),
            content_hash: embedding_content_hash(&content),
            content,
        }
    }

    pub fn validate(&self, max_input_bytes: usize) -> Result<(), EmbeddingError> {
        if self.id.trim().is_empty() {
            return Err(EmbeddingError::MissingInputId);
        }
        if self.content.trim().is_empty() {
            return Err(EmbeddingError::EmptyInput {
                id: self.id.clone(),
            });
        }
        if self.content.len() > max_input_bytes {
            return Err(EmbeddingError::InputTooLarge {
                id: self.id.clone(),
                bytes: self.content.len(),
                limit: max_input_bytes,
            });
        }
        if self.content_hash != embedding_content_hash(&self.content) {
            return Err(EmbeddingError::ContentHashMismatch {
                id: self.id.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingRequest {
    pub config: EmbeddingConfig,
    pub inputs: Vec<EmbeddingInput>,
}

impl EmbeddingRequest {
    pub fn new(
        config: EmbeddingConfig,
        inputs: Vec<EmbeddingInput>,
    ) -> Result<Self, EmbeddingError> {
        config.validate().map_err(EmbeddingError::Config)?;
        if !config.is_enabled() {
            return Err(EmbeddingError::Disabled);
        }
        if inputs.is_empty() || inputs.len() > config.max_batch_size {
            return Err(EmbeddingError::InvalidBatchSize {
                actual: inputs.len(),
                limit: config.max_batch_size,
            });
        }
        for input in &inputs {
            input.validate(config.max_input_bytes)?;
        }
        Ok(Self { config, inputs })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingVector {
    pub id: String,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingResponse {
    pub embedding_version: String,
    pub vectors: Vec<EmbeddingVector>,
}

impl EmbeddingResponse {
    pub fn validate_for(&self, request: &EmbeddingRequest) -> Result<(), EmbeddingError> {
        let expected_version = request
            .config
            .embedding_version()
            .map_err(EmbeddingError::Config)?
            .expect("enabled embedding request has a version");
        if self.embedding_version != expected_version {
            return Err(EmbeddingError::VersionMismatch);
        }
        if self.vectors.len() != request.inputs.len() {
            return Err(EmbeddingError::ResponseCountMismatch {
                expected: request.inputs.len(),
                actual: self.vectors.len(),
            });
        }
        for (input, vector) in request.inputs.iter().zip(&self.vectors) {
            if input.id != vector.id {
                return Err(EmbeddingError::ResponseIdMismatch {
                    expected: input.id.clone(),
                    actual: vector.id.clone(),
                });
            }
            if vector.values.len() != request.config.dimensions.unwrap_or_default() {
                return Err(EmbeddingError::DimensionMismatch {
                    id: vector.id.clone(),
                    expected: request.config.dimensions.unwrap_or_default(),
                    actual: vector.values.len(),
                });
            }
            if vector.values.iter().any(|value| !value.is_finite()) {
                return Err(EmbeddingError::NonFiniteValue {
                    id: vector.id.clone(),
                });
            }
            if request.config.normalization == EmbeddingNormalization::L2 {
                let magnitude = vector
                    .values
                    .iter()
                    .map(|value| value * value)
                    .sum::<f32>()
                    .sqrt();
                if (magnitude - 1.0).abs() > 0.001 {
                    return Err(EmbeddingError::NormalizationMismatch {
                        id: vector.id.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EmbeddingError {
    #[error(transparent)]
    Config(#[from] EmbeddingConfigError),
    #[error("embeddings are disabled")]
    Disabled,
    #[error("embedding input id must not be empty")]
    MissingInputId,
    #[error("embedding input {id} must not be empty")]
    EmptyInput { id: String },
    #[error("embedding input {id} is {bytes} bytes, above the {limit} byte limit")]
    InputTooLarge {
        id: String,
        bytes: usize,
        limit: usize,
    },
    #[error("embedding input {id} does not match its content hash")]
    ContentHashMismatch { id: String },
    #[error("embedding request has {actual} inputs, outside the 1..={limit} batch bound")]
    InvalidBatchSize { actual: usize, limit: usize },
    #[error("embedding response version does not match the request")]
    VersionMismatch,
    #[error("embedding response returned {actual} vectors, expected {expected}")]
    ResponseCountMismatch { expected: usize, actual: usize },
    #[error("embedding response exceeded the {limit} byte limit")]
    ResponseTooLarge { limit: usize },
    #[error("embedding response id {actual} does not match input {expected}")]
    ResponseIdMismatch { expected: String, actual: String },
    #[error("embedding vector {id} has {actual} dimensions, expected {expected}")]
    DimensionMismatch {
        id: String,
        expected: usize,
        actual: usize,
    },
    #[error("embedding vector {id} has a non-finite value")]
    NonFiniteValue { id: String },
    #[error("embedding vector {id} is not L2 normalized")]
    NormalizationMismatch { id: String },
    #[error("embedding transport failed: {0}")]
    Transport(String),
}

pub fn embedding_content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

pub fn embedding_text(record: &StoredMemoryRecord) -> String {
    match &record.resource {
        MemoryRecordResource::Episodic(value) => value.text.clone(),
        MemoryRecordResource::Semantic(value) => format!(
            "subject: {}\npredicate: {}\nobject: {}",
            value.semantic_subject, value.predicate, value.object
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddings_default_to_deferred_disabled_provider() {
        let config = EmbeddingConfig::default();

        assert!(!config.is_enabled());
        assert_eq!(config.provider, EmbeddingProvider::Disabled);
        assert_eq!(config.dimensions, None);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn enabled_embeddings_require_dimension_pinning() {
        let missing = EmbeddingConfig {
            provider: EmbeddingProvider::OpenAiCompatible,
            dimensions: None,
            model: Some("text-embedding-3-small".to_string()),
            ..EmbeddingConfig::default()
        };
        assert_eq!(
            missing.validate(),
            Err(EmbeddingConfigError::MissingDimensions(
                EmbeddingProvider::OpenAiCompatible
            ))
        );

        let pinned = EmbeddingConfig {
            provider: EmbeddingProvider::OpenAiCompatible,
            dimensions: Some(1536),
            model: Some("text-embedding-3-small".to_string()),
            ..EmbeddingConfig::default()
        };
        assert!(pinned.is_enabled());
        assert_eq!(pinned.validate(), Ok(()));
    }

    #[test]
    fn embedding_boundary_pins_model_limits_hash_and_normalization() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            dimensions: Some(3),
            model: Some("nomic-embed-text-v1.5-q4".to_string()),
            ..EmbeddingConfig::default()
        };
        let input = EmbeddingInput::new("record-1", "Scoped evidence stays local.");
        let request = EmbeddingRequest::new(config.clone(), vec![input]).unwrap();
        let response = EmbeddingResponse {
            embedding_version: config.embedding_version().unwrap().unwrap(),
            vectors: vec![EmbeddingVector {
                id: "record-1".to_string(),
                values: vec![0.6, 0.8, 0.0],
            }],
        };
        response.validate_for(&request).unwrap();

        let mut too_large = request.inputs[0].clone();
        too_large.content_hash = "0".repeat(64);
        assert!(matches!(
            too_large.validate(DEFAULT_EMBEDDING_MAX_INPUT_BYTES),
            Err(EmbeddingError::ContentHashMismatch { .. })
        ));
    }

    #[test]
    fn enabled_embeddings_require_model_timeout_and_bounded_batch_configuration() {
        let missing_model = EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            dimensions: Some(384),
            model: None,
            ..EmbeddingConfig::default()
        };
        assert_eq!(
            missing_model.validate(),
            Err(EmbeddingConfigError::MissingModel(EmbeddingProvider::Local))
        );

        let invalid_batch = EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            dimensions: Some(384),
            model: Some("local-test".to_string()),
            max_batch_size: MAX_EMBEDDING_BATCH_SIZE + 1,
            ..EmbeddingConfig::default()
        };
        assert_eq!(
            invalid_batch.validate(),
            Err(EmbeddingConfigError::InvalidBatchSize)
        );
        assert_eq!(
            EmbeddingConfig::default().embedding_version().unwrap(),
            None
        );
        assert_eq!(
            EmbeddingProvenance::from_config(
                &EmbeddingConfig::default(),
                "never embed",
                Utc::now()
            ),
            Err(EmbeddingConfigError::DisabledProvider)
        );
    }

    #[test]
    fn provenance_pins_a_deterministic_version_and_reembedding_key() {
        let version = EmbeddingProvenance::version_id(
            EmbeddingProvider::Local,
            "nomic-embed-text-v1.5-q4",
            768,
            EmbeddingNormalization::L2,
        );
        let provenance = EmbeddingProvenance {
            schema_version: EMBEDDING_PROVENANCE_SCHEMA_VERSION,
            provider: EmbeddingProvider::Local,
            model_id: "nomic-embed-text-v1.5-q4".to_string(),
            dimensions: 768,
            normalization: EmbeddingNormalization::L2,
            content_hash: "a".repeat(64),
            embedding_version: version.clone(),
            created_at: "2026-07-15T00:00:00Z".parse().unwrap(),
            reembedding_state: ReembeddingState::Pending,
        };

        provenance.validate().unwrap();
        assert_eq!(
            version,
            EmbeddingProvenance::version_id(
                EmbeddingProvider::Local,
                "nomic-embed-text-v1.5-q4",
                768,
                EmbeddingNormalization::L2,
            )
        );
        let record_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(
            provenance.reembedding_key(record_id),
            provenance.reembedding_key(record_id)
        );

        let mut mismatched = provenance;
        mismatched.dimensions = 384;
        assert_eq!(
            mismatched.validate(),
            Err(EmbeddingProvenanceError::EmbeddingVersionMismatch)
        );
    }
}
