use std::{sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::StreamExt as _;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tm_memory::{
    EmbeddingClient, EmbeddingConfig, EmbeddingError, EmbeddingInput, EmbeddingRequest,
    EmbeddingResponse, EmbeddingVector, MemoryEmbeddingGeneration, MemoryEmbeddingJobClaim,
    NewMemoryEmbeddingGeneration, embedding_text,
};
use uuid::Uuid;

use crate::store::PostgresStore;
use crate::{Result, ServerError, Store};

/// A deliberately narrow client for an OpenAI-shaped embedding endpoint bound to the local host.
/// It never carries an API key and rejects non-loopback destinations; `openai_compatible` stays a
/// contract-only option until P9 provides destination-scoped egress and opaque secrets.
#[derive(Clone)]
pub struct LocalEmbeddingHttpClient {
    endpoint: Url,
    client: reqwest::Client,
}

#[async_trait]
pub trait MemoryEmbeddingWorker: Send + Sync + 'static {
    async fn tick(&self) -> Result<usize>;
}

pub struct PostgresMemoryEmbeddingWorker {
    store: Arc<PostgresStore>,
    client: Arc<dyn EmbeddingClient>,
    config: EmbeddingConfig,
    owner_subject: String,
    worker_id: Uuid,
    lease_timeout: Duration,
}

impl PostgresMemoryEmbeddingWorker {
    pub fn new(
        store: Arc<PostgresStore>,
        client: Arc<dyn EmbeddingClient>,
        config: EmbeddingConfig,
        owner_subject: impl Into<String>,
    ) -> Result<Self> {
        config
            .validate()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        if config.provider != tm_memory::EmbeddingProvider::Local {
            return Err(ServerError::InvalidRequest(
                "the production embedding worker currently requires provider=local".to_string(),
            ));
        }
        Ok(Self {
            store,
            client,
            config,
            owner_subject: owner_subject.into(),
            worker_id: Uuid::new_v4(),
            lease_timeout: Duration::seconds(60),
        })
    }
}

#[async_trait]
impl MemoryEmbeddingWorker for PostgresMemoryEmbeddingWorker {
    async fn tick(&self) -> Result<usize> {
        let mut completed = 0;
        for scope in self.store.active_memory_scopes(&self.owner_subject).await? {
            let now = Utc::now();
            let generation = self
                .store
                .stage_memory_embedding_generation(
                    NewMemoryEmbeddingGeneration::from_config(
                        &self.owner_subject,
                        &scope,
                        &self.config,
                        now,
                    )
                    .map_err(|error| ServerError::InvalidRequest(error.to_string()))?,
                )
                .await?;
            completed += run_memory_embedding_batch(
                &self.store,
                self.client.as_ref(),
                &generation,
                self.worker_id,
                now,
                self.lease_timeout,
                self.config.clone(),
            )
            .await?;
        }
        Ok(completed)
    }
}

impl LocalEmbeddingHttpClient {
    pub fn new(endpoint: Url) -> Result<Self> {
        if endpoint.scheme() != "http"
            || !is_loopback_host(endpoint.host_str())
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ServerError::InvalidRequest(
                "local embedding endpoint must be plain HTTP on localhost without credentials, query, or fragment"
                    .to_string(),
            ));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ServerError::Store(error.to_string()))?;
        Ok(Self { endpoint, client })
    }
}

#[async_trait]
impl EmbeddingClient for LocalEmbeddingHttpClient {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, EmbeddingError> {
        if request.config.provider != tm_memory::EmbeddingProvider::Local {
            return Err(EmbeddingError::Transport(
                "local embedding transport only accepts the local provider".to_string(),
            ));
        }
        let model = request
            .config
            .model
            .clone()
            .expect("validated embedding request pins a model");
        let response = self
            .client
            .post(self.endpoint.clone())
            .timeout(StdDuration::from_millis(request.config.timeout_ms))
            .json(&LocalEmbeddingRequest {
                model,
                input: request.inputs.iter().map(|input| &input.content).collect(),
            })
            .send()
            .await
            .map_err(|error| EmbeddingError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| EmbeddingError::Transport(error.to_string()))?;
        let dimensions = request
            .config
            .dimensions
            .expect("validated embedding request pins dimensions");
        let limit = request
            .inputs
            .len()
            .checked_mul(dimensions)
            .and_then(|values| values.checked_mul(32))
            .and_then(|vectors| (64 * 1024usize).checked_add(vectors))
            .ok_or(EmbeddingError::ResponseTooLarge { limit: 64 * 1024 })?;
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(EmbeddingError::ResponseTooLarge { limit });
        }
        let mut body = Vec::with_capacity(limit.min(256 * 1024));
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| EmbeddingError::Transport(error.to_string()))?;
            if body
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > limit)
            {
                return Err(EmbeddingError::ResponseTooLarge { limit });
            }
            body.extend_from_slice(&chunk);
        }
        let response: LocalEmbeddingResponse = serde_json::from_slice(&body)
            .map_err(|error| EmbeddingError::Transport(error.to_string()))?;
        let mut data = response.data;
        data.sort_by_key(|item| item.index);
        if data.len() != request.inputs.len() {
            return Err(EmbeddingError::ResponseCountMismatch {
                expected: request.inputs.len(),
                actual: data.len(),
            });
        }
        for (expected, item) in data.iter().enumerate() {
            if item.index != expected {
                return Err(EmbeddingError::ResponseIdMismatch {
                    expected: expected.to_string(),
                    actual: item.index.to_string(),
                });
            }
        }
        let vectors = data
            .into_iter()
            .enumerate()
            .map(|(index, item)| EmbeddingVector {
                id: request
                    .inputs
                    .get(index)
                    .map(|input| input.id.clone())
                    .unwrap_or_else(|| format!("unexpected-{index}")),
                values: item.embedding,
            })
            .collect();
        Ok(EmbeddingResponse {
            embedding_version: request
                .config
                .embedding_version()
                .map_err(EmbeddingError::Config)?
                .expect("enabled embedding request has a version"),
            vectors,
        })
    }
}

#[derive(Serialize)]
struct LocalEmbeddingRequest<'a> {
    model: String,
    input: Vec<&'a String>,
}

#[derive(Deserialize)]
struct LocalEmbeddingResponse {
    data: Vec<LocalEmbeddingData>,
}

#[derive(Deserialize)]
struct LocalEmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

/// Runs at most one durable embedding batch. Provider loss releases leases and leaves lexical
/// recall unchanged; the supervised runtime repeatedly invokes this bounded tick.
pub async fn run_memory_embedding_batch(
    store: &PostgresStore,
    client: &dyn EmbeddingClient,
    generation: &MemoryEmbeddingGeneration,
    worker_id: Uuid,
    now: DateTime<Utc>,
    lease_timeout: Duration,
    config: EmbeddingConfig,
) -> Result<usize> {
    config
        .validate()
        .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
    if config.provider != generation.provider
        || config.model.as_deref() != Some(generation.model_id.as_str())
        || config.dimensions != Some(generation.dimensions)
        || config.normalization != generation.normalization
        || config
            .embedding_version()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?
            .as_deref()
            != Some(generation.embedding_version.as_str())
    {
        return Err(ServerError::InvalidRequest(
            "embedding worker config does not match its staged generation".to_string(),
        ));
    }
    let leases = store
        .claim_memory_embedding_jobs(&MemoryEmbeddingJobClaim {
            owner_subject: generation.owner_subject.clone(),
            memory_scope: generation.memory_scope.clone(),
            embedding_version: generation.embedding_version.clone(),
            owner_id: worker_id,
            now,
            lease_timeout,
            limit: config.max_batch_size,
        })
        .await?;
    if leases.is_empty() {
        return Ok(0);
    }
    let mut valid = Vec::with_capacity(leases.len());
    for lease in leases {
        let record = match store
            .memory_record(
                &lease.job.owner_subject,
                &lease.job.memory_scope,
                lease.job.record_kind,
                lease.job.record_id,
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                let _ = store.retry_memory_embedding_job(&lease, now).await;
                tracing::warn!(job_id = %lease.job.id, %error, "could not resolve embedding job record");
                continue;
            }
        };
        let input = EmbeddingInput::new(lease.job.id.to_string(), embedding_text(&record));
        if let Err(error) = input.validate(config.max_input_bytes) {
            let failure_code = if matches!(error, EmbeddingError::InputTooLarge { .. }) {
                "input_too_large"
            } else {
                "invalid_input"
            };
            store
                .fail_memory_embedding_job(&lease, failure_code, &error.to_string(), now)
                .await?;
            continue;
        }
        valid.push((lease, input));
    }
    if valid.is_empty() {
        store
            .reconcile_memory_embedding_generation(generation, now)
            .await?;
        return Ok(0);
    }
    let request = match EmbeddingRequest::new(
        config,
        valid.iter().map(|(_, input)| input.clone()).collect(),
    ) {
        Ok(request) => request,
        Err(error) => {
            for (lease, _) in &valid {
                store
                    .fail_memory_embedding_job(lease, "invalid_input", &error.to_string(), now)
                    .await?;
            }
            store
                .reconcile_memory_embedding_generation(generation, now)
                .await?;
            return Err(ServerError::InvalidRequest(error.to_string()));
        }
    };
    let response = match client.embed(request.clone()).await {
        Ok(response) => response,
        Err(error) => {
            for (lease, _) in &valid {
                let _ = store.retry_memory_embedding_job(lease, now).await;
            }
            return Err(ServerError::Store(
                tm_memory::redact_dream_text(&error.to_string()).text,
            ));
        }
    };
    if let Err(error) = response.validate_for(&request) {
        for (lease, _) in &valid {
            let _ = store.retry_memory_embedding_job(lease, now).await;
        }
        return Err(ServerError::InvalidRequest(error.to_string()));
    }
    for ((lease, _), vector) in valid.iter().zip(response.vectors) {
        store
            .complete_memory_embedding_job(lease, &vector.values, now)
            .await?;
    }
    store
        .reconcile_memory_embedding_generation(generation, now)
        .await?;
    Ok(valid.len())
}

fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("localhost") | Some("127.0.0.1") | Some("::1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_embedding_client_rejects_egress_destinations() {
        assert!(
            LocalEmbeddingHttpClient::new(
                Url::parse("https://api.example.invalid/v1/embeddings").unwrap()
            )
            .is_err()
        );
        assert!(
            LocalEmbeddingHttpClient::new(
                Url::parse("http://127.0.0.1:8080/v1/embeddings").unwrap()
            )
            .is_ok()
        );
        for endpoint in [
            "http://secret@127.0.0.1:8080/v1/embeddings",
            "http://127.0.0.1:8080/v1/embeddings?token=secret",
            "http://127.0.0.1:8080/v1/embeddings#secret",
        ] {
            assert!(LocalEmbeddingHttpClient::new(Url::parse(endpoint).unwrap()).is_err());
        }
    }

    #[tokio::test]
    async fn local_embedding_client_never_follows_redirects() {
        let followed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let target_followed = Arc::clone(&followed);
        let app = axum::Router::new()
            .route(
                "/redirect",
                axum::routing::post(|| async { axum::response::Redirect::temporary("/target") }),
            )
            .route(
                "/target",
                axum::routing::post(move || {
                    let target_followed = Arc::clone(&target_followed);
                    async move {
                        target_followed.store(true, std::sync::atomic::Ordering::SeqCst);
                        axum::Json(serde_json::json!({
                            "data": [{"index": 0, "embedding": [1.0, 0.0]}]
                        }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = LocalEmbeddingHttpClient::new(
            Url::parse(&format!("http://{address}/redirect")).unwrap(),
        )
        .unwrap();
        let request = EmbeddingRequest::new(
            EmbeddingConfig {
                provider: tm_memory::EmbeddingProvider::Local,
                dimensions: Some(2),
                model: Some("redirect-fixture".to_string()),
                ..EmbeddingConfig::default()
            },
            vec![EmbeddingInput::new("query", "redirects stay local")],
        )
        .unwrap();

        assert!(client.embed(request).await.is_err());
        assert!(!followed.load(std::sync::atomic::Ordering::SeqCst));
        server.abort();
    }

    #[tokio::test]
    async fn local_embedding_client_rejects_chunked_responses_above_the_derived_cap() {
        let app = axum::Router::new().route(
            "/embeddings",
            axum::routing::post(|| async {
                let chunks = vec![
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![b'x'; 40_000])),
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![b'y'; 40_000])),
                ];
                axum::response::Response::builder()
                    .body(axum::body::Body::from_stream(futures::stream::iter(chunks)))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = LocalEmbeddingHttpClient::new(
            Url::parse(&format!("http://{address}/embeddings")).unwrap(),
        )
        .unwrap();
        let request = EmbeddingRequest::new(
            EmbeddingConfig {
                provider: tm_memory::EmbeddingProvider::Local,
                dimensions: Some(2),
                model: Some("response-cap-fixture".to_string()),
                ..EmbeddingConfig::default()
            },
            vec![EmbeddingInput::new("query", "bounded response")],
        )
        .unwrap();

        assert!(matches!(
            client.embed(request).await,
            Err(EmbeddingError::ResponseTooLarge { .. })
        ));
        server.abort();
    }
}
