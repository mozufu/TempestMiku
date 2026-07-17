use std::sync::Arc;

use super::{
    DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS, DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES,
    DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT,
};

mod access;
mod content;
mod entries;
mod handler;
mod render;
mod uri;

#[derive(Clone)]
pub struct MemoryResourceHandler<S> {
    store: Arc<S>,
    subject: String,
    scope: String,
    recall_limit: usize,
    prompt_budget_tokens: usize,
    preview_bytes: usize,
}

impl<S> MemoryResourceHandler<S> {
    pub fn new(store: Arc<S>, subject: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            store,
            subject: subject.into(),
            scope: scope.into(),
            recall_limit: DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT,
            prompt_budget_tokens: DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS,
            preview_bytes: DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES,
        }
    }

    pub fn with_recall_limit(mut self, recall_limit: usize) -> Self {
        self.recall_limit = recall_limit;
        self
    }

    pub fn with_prompt_budget_tokens(mut self, prompt_budget_tokens: usize) -> Self {
        self.prompt_budget_tokens = prompt_budget_tokens;
        self
    }

    pub fn with_preview_bytes(mut self, preview_bytes: usize) -> Self {
        self.preview_bytes = preview_bytes;
        self
    }
}
