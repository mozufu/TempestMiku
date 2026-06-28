use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryContext {
    pub profile_facts: Vec<String>,
    pub recall_chunks: Vec<String>,
}

impl MemoryContext {
    pub fn is_empty(&self) -> bool {
        self.profile_facts.is_empty() && self.recall_chunks.is_empty()
    }

    pub fn render_prompt_block(&self) -> String {
        let mut lines = Vec::new();
        if !self.profile_facts.is_empty() {
            lines.push("Profile facts:".to_string());
            lines.extend(self.profile_facts.iter().map(|fact| format!("- {fact}")));
        }
        if !self.recall_chunks.is_empty() {
            lines.push("Recall chunks:".to_string());
            lines.extend(self.recall_chunks.iter().map(|chunk| format!("- {chunk}")));
        }
        lines.join("\n")
    }
}

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
}

impl<S> StoreMemoryProvider<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            recall_limit: 5,
        }
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
        Ok(MemoryContext {
            profile_facts: facts
                .into_iter()
                .map(|fact| format!("{} {} {}", fact.subject, fact.predicate, fact.object))
                .collect(),
            recall_chunks: chunks.into_iter().map(|chunk| chunk.text).collect(),
        })
    }
}
