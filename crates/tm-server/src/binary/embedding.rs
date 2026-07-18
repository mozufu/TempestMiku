use std::sync::Arc;

use tm_server::LocalEmbeddingHttpClient;

use super::{
    BoxError,
    config::{optional_env_parse, required_env},
};

pub(super) struct EmbeddingSetup {
    pub(super) config: tm_memory::EmbeddingConfig,
    pub(super) client: Option<Arc<dyn tm_memory::EmbeddingClient>>,
}

pub(super) fn embedding_setup_from_env() -> Result<EmbeddingSetup, BoxError> {
    let provider = std::env::var("TM_MEMORY_EMBEDDING_PROVIDER")
        .unwrap_or_else(|_| "disabled".to_string())
        .trim()
        .to_ascii_lowercase();
    if matches!(provider.as_str(), "" | "disabled" | "none") {
        return Ok(EmbeddingSetup {
            config: tm_memory::EmbeddingConfig::default(),
            client: None,
        });
    }
    if provider == "openai_compatible" {
        return Err(
            "TM_MEMORY_EMBEDDING_PROVIDER=openai_compatible waits for the P9 egress/secret boundary"
                .into(),
        );
    }
    if provider != "local" {
        return Err(format!(
            "unsupported TM_MEMORY_EMBEDDING_PROVIDER={provider}; expected disabled or local"
        )
        .into());
    }
    let model = required_env("TM_MEMORY_EMBEDDING_MODEL")?;
    let dimensions = required_env("TM_MEMORY_EMBEDDING_DIMENSIONS")?.parse::<usize>()?;
    let endpoint = reqwest::Url::parse(&required_env("TM_MEMORY_EMBEDDING_ENDPOINT")?)?;
    let normalization = match std::env::var("TM_MEMORY_EMBEDDING_NORMALIZATION")
        .unwrap_or_else(|_| "l2".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "l2" => tm_memory::EmbeddingNormalization::L2,
        "none" => tm_memory::EmbeddingNormalization::None,
        other => {
            return Err(format!(
                "unsupported TM_MEMORY_EMBEDDING_NORMALIZATION={other}; expected l2 or none"
            )
            .into());
        }
    };
    let config = tm_memory::EmbeddingConfig {
        provider: tm_memory::EmbeddingProvider::Local,
        dimensions: Some(dimensions),
        model: Some(model),
        normalization,
        timeout_ms: optional_env_parse("TM_MEMORY_EMBEDDING_TIMEOUT_MS", 5_000)?,
        max_batch_size: optional_env_parse("TM_MEMORY_EMBEDDING_MAX_BATCH_SIZE", 32)?,
        max_input_bytes: optional_env_parse("TM_MEMORY_EMBEDDING_MAX_INPUT_BYTES", 16 * 1024)?,
    };
    config.validate()?;
    let client: Arc<dyn tm_memory::EmbeddingClient> =
        Arc::new(LocalEmbeddingHttpClient::new(endpoint)?);
    Ok(EmbeddingSetup {
        config,
        client: Some(client),
    })
}
