use serde::{Deserialize, Serialize};

use crate::EmbeddingConfig;

use super::MAX_PGVECTOR_INDEX_DIMENSIONS;

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
