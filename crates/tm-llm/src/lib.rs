//! A streaming-first, OpenAI-compatible chat client.
//!
//! [`OpenAiClient`] implements [`tm_core::LlmClient`]. Only [`chat_stream`](OpenAiClient) is
//! native — it POSTs `stream: true` and adapts the SSE response into a [`StreamEvent`] stream;
//! the non-streaming `chat` comes free from the trait's default (drain + assemble).

mod sse;
mod wire;

use async_trait::async_trait;
use futures::stream::BoxStream;

use tm_core::{ChatRequest, Error, LlmClient, Result, StreamEvent};

/// Connection settings for an OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL including the version segment, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Bearer token, if the endpoint requires one.
    pub api_key: Option<String>,
    /// Request `stream_options.include_usage`. Most servers accept it; disable for the few
    /// that reject unknown fields.
    pub include_usage: bool,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            include_usage: true,
        }
    }
}

/// A chat client speaking the OpenAI streaming protocol.
pub struct OpenAiClient {
    http: reqwest::Client,
    cfg: OpenAiConfig,
}

impl OpenAiClient {
    pub fn new(cfg: OpenAiConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Llm(e.to_string()))?;
        Ok(Self { http, cfg })
    }

    /// Build from the environment:
    /// `OPENAI_BASE_URL` (default `https://api.openai.com/v1`), `OPENAI_API_KEY`,
    /// `OPENAI_STREAM_USAGE` (`0`/`false`/`no` disables `include_usage`).
    pub fn from_env() -> Result<Self> {
        let mut cfg = OpenAiConfig::default();
        if let Ok(base) = std::env::var("OPENAI_BASE_URL")
            && !base.trim().is_empty()
        {
            cfg.base_url = base;
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY")
            && !key.trim().is_empty()
        {
            cfg.api_key = Some(key);
        }
        if let Ok(v) = std::env::var("OPENAI_STREAM_USAGE") {
            cfg.include_usage = !matches!(v.trim(), "0" | "false" | "no");
        }
        Self::new(cfg)
    }

    pub fn config(&self) -> &OpenAiConfig {
        &self.cfg
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let body = wire::build_body(req, self.cfg.include_usage);
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );

        let mut builder = self.http.post(url).json(&body);
        if let Some(key) = &self.cfg.api_key {
            builder = builder.bearer_auth(key);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| Error::Llm(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("HTTP {status}: {}", text.trim())));
        }

        Ok(sse::events(resp.bytes_stream()))
    }
}
