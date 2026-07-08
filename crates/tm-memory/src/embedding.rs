use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProvider {
    Disabled,
    Local,
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub provider: EmbeddingProvider,
    pub dimensions: Option<usize>,
    pub model: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EmbeddingConfigError {
    #[error("embedding dimensions must be pinned when provider is {0:?}")]
    MissingDimensions(EmbeddingProvider),
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProvider::Disabled,
            dimensions: None,
            model: None,
        }
    }
}

impl EmbeddingConfig {
    pub fn validate(&self) -> Result<(), EmbeddingConfigError> {
        if self.provider != EmbeddingProvider::Disabled && self.dimensions.unwrap_or(0) == 0 {
            return Err(EmbeddingConfigError::MissingDimensions(self.provider));
        }
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.provider != EmbeddingProvider::Disabled
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
        };
        assert!(pinned.is_enabled());
        assert_eq!(pinned.validate(), Ok(()));
    }
}
