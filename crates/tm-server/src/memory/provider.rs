use std::sync::Arc;

use async_trait::async_trait;
use tm_memory::{
    DenseRecallQuery, DenseRecallStatus, EmbeddingClient, EmbeddingConfig, EmbeddingInput,
    EmbeddingRequest, HybridRecallRequest,
};

use crate::{Result, ServerError, Store};

use super::{DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS, MemoryContext};

#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    async fn context_for_turn(
        &self,
        subject: &str,
        scope: &str,
        query: &str,
    ) -> Result<MemoryContext>;
}

#[derive(Clone)]
pub struct StoreMemoryProvider<S> {
    store: Arc<S>,
    recall_limit: usize,
    summary_limit: usize,
    prompt_budget_tokens: usize,
    embeddings: Option<EmbeddingRuntime>,
}

#[derive(Clone)]
struct EmbeddingRuntime {
    config: EmbeddingConfig,
    client: Arc<dyn EmbeddingClient>,
}

impl<S> StoreMemoryProvider<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            recall_limit: 5,
            summary_limit: 3,
            prompt_budget_tokens: DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS,
            embeddings: None,
        }
    }

    pub fn with_recall_limit(mut self, recall_limit: usize) -> Self {
        self.recall_limit = recall_limit;
        self
    }

    pub fn with_summary_limit(mut self, summary_limit: usize) -> Self {
        self.summary_limit = summary_limit;
        self
    }

    pub fn with_prompt_budget_tokens(mut self, prompt_budget_tokens: usize) -> Self {
        self.prompt_budget_tokens = prompt_budget_tokens;
        self
    }

    pub fn with_embeddings(
        mut self,
        config: EmbeddingConfig,
        client: Arc<dyn EmbeddingClient>,
    ) -> Result<Self> {
        config
            .validate()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        if !config.is_enabled() {
            return Err(ServerError::InvalidRequest(
                "an embedding client requires an enabled provider".to_string(),
            ));
        }
        self.embeddings = Some(EmbeddingRuntime { config, client });
        Ok(self)
    }

    pub fn embeddings_enabled(&self) -> bool {
        self.embeddings.is_some()
    }
}

#[async_trait]
impl<S> MemoryProvider for StoreMemoryProvider<S>
where
    S: Store,
{
    async fn context_for_turn(
        &self,
        subject: &str,
        scope: &str,
        query: &str,
    ) -> Result<MemoryContext> {
        self.store
            .ensure_memory_scope_active(subject, scope)
            .await?;
        let Some(embeddings) = &self.embeddings else {
            return self.legacy_context(subject, scope, query).await;
        };
        let Some(generation) = self
            .store
            .active_memory_embedding_generation(subject, scope)
            .await?
        else {
            return self
                .typed_lexical_fallback(
                    subject,
                    scope,
                    query,
                    "no_active_embedding_generation",
                    None,
                )
                .await;
        };
        if generation.provider != embeddings.config.provider
            || embeddings.config.model.as_deref() != Some(generation.model_id.as_str())
            || embeddings.config.dimensions != Some(generation.dimensions)
            || embeddings.config.normalization != generation.normalization
        {
            return self
                .typed_lexical_fallback(
                    subject,
                    scope,
                    query,
                    "embedding_generation_config_mismatch",
                    Some(generation.embedding_version),
                )
                .await;
        }

        let request = match EmbeddingRequest::new(
            embeddings.config.clone(),
            vec![EmbeddingInput::new("turn-query", query)],
        ) {
            Ok(request) => request,
            Err(_) => {
                return self
                    .typed_lexical_fallback(
                        subject,
                        scope,
                        query,
                        "embedding_query_rejected",
                        Some(generation.embedding_version),
                    )
                    .await;
            }
        };
        let response = match embeddings.client.embed(request.clone()).await {
            Ok(response) if response.validate_for(&request).is_ok() => response,
            Ok(_) => {
                return self
                    .typed_lexical_fallback(
                        subject,
                        scope,
                        query,
                        "embedding_response_invalid",
                        Some(generation.embedding_version),
                    )
                    .await;
            }
            Err(_) => {
                return self
                    .typed_lexical_fallback(
                        subject,
                        scope,
                        query,
                        "embedding_provider_unavailable",
                        Some(generation.embedding_version),
                    )
                    .await;
            }
        };
        let vector = response
            .vectors
            .first()
            .expect("validated one-input embedding response has one vector");
        let recall_request = self.recall_request(subject, scope);
        let dense_query = DenseRecallQuery {
            embedding_version: generation.embedding_version.clone(),
            snapshot_revision: generation.snapshot_revision,
            values: vector.values.clone(),
        };
        let result = match self
            .store
            .memory_hybrid_candidates(&recall_request, query, Some(&dense_query))
            .await
        {
            Ok(candidates) => candidates,
            Err(ServerError::InvalidRequest(_)) => {
                return self
                    .typed_lexical_fallback(
                        subject,
                        scope,
                        query,
                        "dense_query_incompatible",
                        Some(generation.embedding_version),
                    )
                    .await;
            }
            Err(error) => return Err(error),
        };
        let degraded_reason = match result.dense_status {
            DenseRecallStatus::GenerationChanged => Some("embedding_generation_changed"),
            DenseRecallStatus::Unavailable => Some("dense_backend_unavailable"),
            DenseRecallStatus::NotRequested | DenseRecallStatus::Applied => None,
        };
        let mut facts = self.hybrid_profile_facts(subject).await?;
        facts.truncate(self.recall_limit);
        let mut summaries = self
            .store
            .memory_summaries(scope, self.summary_limit)
            .await?;
        let mut candidates = result.candidates;
        candidates.truncate(self.recall_limit);
        summaries.truncate(self.summary_limit);
        let context = MemoryContext::from_hybrid_candidates_with_profile_facts_and_summaries(
            subject,
            scope,
            facts,
            summaries,
            candidates,
            self.prompt_budget_tokens,
            Some(generation.embedding_version),
        );
        Ok(match degraded_reason {
            Some(reason) => context.mark_lexical_fallback(reason),
            None => context,
        })
    }
}

impl<S> StoreMemoryProvider<S>
where
    S: Store,
{
    async fn legacy_context(
        &self,
        subject: &str,
        scope: &str,
        query: &str,
    ) -> Result<MemoryContext> {
        let chunks = self
            .store
            .recall_chunks(scope, query, self.recall_limit)
            .await?;
        let facts = self.store.profile_facts(subject).await?;
        let summaries = self
            .store
            .memory_summaries(scope, self.summary_limit)
            .await?;
        Ok(MemoryContext::from_records_with_summaries(
            subject,
            scope,
            facts,
            summaries,
            chunks,
            self.prompt_budget_tokens,
        ))
    }

    async fn typed_lexical_fallback(
        &self,
        subject: &str,
        scope: &str,
        query: &str,
        reason: &str,
        embedding_version: Option<String>,
    ) -> Result<MemoryContext> {
        let request = self.recall_request(subject, scope);
        let result = match self
            .store
            .memory_hybrid_candidates(&request, query, None)
            .await
        {
            Ok(candidates) => candidates,
            Err(ServerError::Store(message))
                if message == "hybrid memory recall is not implemented" =>
            {
                return Ok(self
                    .legacy_context(subject, scope, query)
                    .await?
                    .mark_lexical_fallback(reason));
            }
            Err(error) => return Err(error),
        };
        let mut facts = self.hybrid_profile_facts(subject).await?;
        facts.truncate(self.recall_limit);
        let mut summaries = self
            .store
            .memory_summaries(scope, self.summary_limit)
            .await?;
        let mut candidates = result.candidates;
        candidates.truncate(self.recall_limit);
        summaries.truncate(self.summary_limit);
        Ok(
            MemoryContext::from_hybrid_candidates_with_profile_facts_and_summaries(
                subject,
                scope,
                facts,
                summaries,
                candidates,
                self.prompt_budget_tokens,
                embedding_version,
            )
            .mark_lexical_fallback(reason),
        )
    }

    fn recall_request(&self, subject: &str, scope: &str) -> HybridRecallRequest {
        let candidate_limit = self.recall_limit.clamp(
            tm_memory::DEFAULT_HYBRID_CANDIDATE_LIMIT,
            tm_memory::MAX_HYBRID_CANDIDATE_LIMIT,
        );
        HybridRecallRequest {
            owner_subject: subject.to_string(),
            memory_scope: scope.to_string(),
            candidate_limit,
            top_k: self.recall_limit.min(candidate_limit).max(1),
            ..HybridRecallRequest::default()
        }
    }

    async fn hybrid_profile_facts(
        &self,
        subject: &str,
    ) -> Result<Vec<crate::store::ProfileFactRecord>> {
        let facts = self.store.profile_facts(subject).await?;
        let mut eligible = Vec::with_capacity(facts.len());
        for fact in facts {
            match self
                .store
                .memory_record(
                    subject,
                    "global",
                    tm_memory::MemoryRecordKind::Semantic,
                    fact.id,
                )
                .await
            {
                Ok(record)
                    if record.resource.status() == tm_memory::MemoryRecordStatus::Active
                        && record.resource.effective_to().is_none() =>
                {
                    eligible.push(fact);
                }
                Ok(_) => {}
                Err(ServerError::NotFound(_)) => eligible.push(fact),
                Err(ServerError::Store(message))
                    if message.contains("durable P8 memory record lookup is not implemented") =>
                {
                    eligible.push(fact);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(eligible)
    }
}
