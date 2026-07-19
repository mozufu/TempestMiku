//! A streaming-first, OpenAI-compatible chat client.
//!
//! [`OpenAiClient`] implements [`tm_core::LlmClient`]. Only [`chat_stream`](OpenAiClient) is
//! native — it POSTs `stream: true` and adapts the SSE response into a [`StreamEvent`] stream;
//! the non-streaming `chat` comes free from the trait's default (drain + assemble).

mod sse;
mod wire;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::{fmt, time::Duration};

use tm_core::{ChatRequest, Error, LlmClient, Result, StreamEvent};

/// Connection settings for an OpenAI-compatible endpoint.
#[derive(Clone)]
pub struct OpenAiConfig {
    /// Base URL including the version segment, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Bearer token, if the endpoint requires one.
    pub api_key: Option<String>,
    /// Request `stream_options.include_usage`. Most servers accept it; disable for the few
    /// that reject unknown fields.
    pub include_usage: bool,
    /// Optional OpenAI-compatible Chat Completions reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Reuse idle HTTP connections between completions. Some OpenAI-compatible reverse proxies
    /// require a fresh connection for each streamed request; disable this only for those endpoints.
    pub reuse_connections: bool,
    /// TCP/TLS connection deadline.
    pub connect_timeout: Duration,
    /// Maximum idle time between streamed response chunks.
    pub read_timeout: Duration,
    /// Total request deadline, including the streamed response body.
    pub request_timeout: Duration,
    /// Maximum bytes in one SSE line.
    pub max_sse_line_bytes: usize,
    /// Maximum bytes accepted for one completion stream.
    pub max_stream_bytes: usize,
    /// Maximum bytes retained from a non-success response body.
    pub max_error_body_bytes: usize,
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base_url = tm_memory::redact_dream_text(&self.base_url).text;
        f.debug_struct("OpenAiConfig")
            .field("base_url", &base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("include_usage", &self.include_usage)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("reuse_connections", &self.reuse_connections)
            .field("connect_timeout", &self.connect_timeout)
            .field("read_timeout", &self.read_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("max_sse_line_bytes", &self.max_sse_line_bytes)
            .field("max_stream_bytes", &self.max_stream_bytes)
            .field("max_error_body_bytes", &self.max_error_body_bytes)
            .finish()
    }
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            include_usage: true,
            reasoning_effort: None,
            reuse_connections: true,
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(300),
            max_sse_line_bytes: 1024 * 1024,
            max_stream_bytes: 4 * 1024 * 1024,
            max_error_body_bytes: 64 * 1024,
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
        validate_config(&cfg)?;
        let mut http = reqwest::Client::builder()
            .connect_timeout(cfg.connect_timeout)
            .read_timeout(cfg.read_timeout)
            .timeout(cfg.request_timeout);
        if !cfg.reuse_connections {
            http = http.pool_max_idle_per_host(0);
        }
        let http = http
            .build()
            .map_err(|error| Error::Llm(redact_transport_error(error)))?;
        Ok(Self { http, cfg })
    }

    /// Build from the environment:
    /// `OPENAI_BASE_URL` (default `https://api.openai.com/v1`), `OPENAI_API_KEY`,
    /// `OPENAI_STREAM_USAGE` (`0`/`false`/`no` disables `include_usage`), and
    /// `OPENAI_REASONING_EFFORT`, and `OPENAI_CONNECTION_REUSE` (`0`/`false`/`no` disables the
    /// idle connection pool for endpoints with broken keep-alive behavior).
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
        if let Ok(value) = std::env::var("OPENAI_REASONING_EFFORT")
            && !value.trim().is_empty()
        {
            cfg.reasoning_effort = Some(value.trim().to_ascii_lowercase());
        }
        if let Ok(value) = std::env::var("OPENAI_CONNECTION_REUSE")
            && !value.trim().is_empty()
        {
            cfg.reuse_connections = parse_connection_reuse(&value)?;
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
        let body = wire::build_body(
            req,
            self.cfg.include_usage,
            self.cfg.reasoning_effort.as_deref(),
        );
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
            .map_err(|error| Error::Llm(redact_transport_error(error)))?;

        let status = resp.status();
        if !status.is_success() {
            let text = redact_error_text(
                bounded_response_text(resp, self.cfg.max_error_body_bytes).await,
                self.cfg.api_key.as_deref(),
            );
            return Err(Error::Llm(format!("HTTP {status}: {}", text.trim())));
        }

        Ok(sse::events(
            resp.bytes_stream(),
            sse::SseLimits {
                max_line_bytes: self.cfg.max_sse_line_bytes,
                max_stream_bytes: self.cfg.max_stream_bytes,
            },
        ))
    }
}

fn redact_error_text(mut text: String, configured_api_key: Option<&str>) -> String {
    if let Some(api_key) = configured_api_key {
        text = text.replace(api_key, "[REDACTED_SECRET]");
    }
    tm_memory::redact_dream_text(&text).text
}

pub(crate) fn redact_transport_error(error: impl ToString) -> String {
    redact_error_text(error.to_string(), None)
}

fn validate_config(cfg: &OpenAiConfig) -> Result<()> {
    if cfg.connect_timeout.is_zero()
        || cfg.read_timeout.is_zero()
        || cfg.request_timeout.is_zero()
        || cfg.max_sse_line_bytes == 0
        || cfg.max_stream_bytes < cfg.max_sse_line_bytes
        || cfg.max_error_body_bytes == 0
    {
        return Err(Error::Llm("invalid OpenAI client limits".to_string()));
    }
    if let Some(effort) = cfg.reasoning_effort.as_deref()
        && !matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        )
    {
        return Err(Error::Llm(format!(
            "invalid OpenAI reasoning effort {effort:?}"
        )));
    }
    Ok(())
}

fn parse_connection_reuse(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(Error::Llm(format!(
            "invalid OPENAI_CONNECTION_REUSE {other:?}; expected a boolean"
        ))),
    }
}

async fn bounded_response_text(resp: reqwest::Response, cap: usize) -> String {
    let mut stream = resp.bytes_stream();
    let mut body = Vec::with_capacity(cap.min(8 * 1024));
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        let remaining = cap.saturating_sub(body.len());
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() == cap {
            truncated = true;
            break;
        }
    }
    let mut text = String::from_utf8_lossy(&body).into_owned();
    if truncated {
        text.push_str("\n… error body truncated …");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_api_key() {
        let cfg = OpenAiConfig {
            api_key: Some("super-secret-token".to_string()),
            base_url: "https://owner:provider-password@example.test/v1".to_string(),
            ..OpenAiConfig::default()
        };
        let debug = format!("{cfg:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-token"));
        assert!(!debug.contains("provider-password"));
    }

    #[test]
    fn rejects_invalid_limits() {
        let cfg = OpenAiConfig {
            max_stream_bytes: 10,
            max_sse_line_bytes: 11,
            ..OpenAiConfig::default()
        };
        assert!(OpenAiClient::new(cfg).is_err());
    }

    #[test]
    fn rejects_unknown_reasoning_effort() {
        let cfg = OpenAiConfig {
            reasoning_effort: Some("turbo".to_string()),
            ..OpenAiConfig::default()
        };
        assert!(OpenAiClient::new(cfg).is_err());
    }

    #[test]
    fn parses_connection_reuse_strictly() {
        assert!(parse_connection_reuse("yes").unwrap());
        assert!(!parse_connection_reuse("OFF").unwrap());
        assert!(parse_connection_reuse("sometimes").is_err());
    }

    #[test]
    fn provider_error_text_uses_the_shared_secret_redactor() {
        let configured = "configured-opaque-secret";
        let body = format!(
            "Authorization: Bearer opaque-token-123456\nprovider=sk-testsecret123456\nconfigured={configured}"
        );
        let redacted = redact_error_text(body, Some(configured));
        assert!(!redacted.contains("opaque-token-123456"));
        assert!(!redacted.contains("sk-testsecret123456"));
        assert!(!redacted.contains(configured));

        let transport = redact_transport_error(
            "request failed for https://owner:provider-password@example.test/v1",
        );
        assert!(!transport.contains("provider-password"));
    }
}
