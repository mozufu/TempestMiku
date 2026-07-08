use std::sync::Arc;

use async_trait::async_trait;

use crate::{Result, Store};

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
}

impl<S> StoreMemoryProvider<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            recall_limit: 5,
            summary_limit: 3,
            prompt_budget_tokens: DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS,
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
        let facts = self.store.profile_facts(subject).await?;
        let chunks = self
            .store
            .recall_chunks(scope, query, self.recall_limit)
            .await?;
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
}
