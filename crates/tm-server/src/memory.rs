mod context;
mod embedding;
mod evaluation;
mod proposal;
mod provider;
mod resource;
mod state_capture;
mod util;

#[cfg(test)]
mod tests;

pub const DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS: usize = 1_600;
pub const DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES: usize = 512;
pub const DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT: usize = 20;

pub use context::{
    MemoryCandidateTrace, MemoryContext, MemoryPromptBudget, MemoryPromptItem, MemoryRetrievalMode,
    MemoryRetrievalTrace,
};
pub use embedding::{
    LocalEmbeddingHttpClient, MemoryEmbeddingWorker, PostgresMemoryEmbeddingWorker,
    run_memory_embedding_batch,
};
pub use evaluation::{
    IN_MEMORY_LEXICAL_BASELINE_MODE, POSTGRES_HYBRID_RECALL_MODE, POSTGRES_LEXICAL_BASELINE_MODE,
    evaluate_lexical_recall_baseline, evaluate_memory_provider_recall,
};
pub use proposal::{
    MemoryRecordRef, MemoryWriteKind, MemoryWriteProposal, MemoryWriteStatus,
    ProfileFactProposalInput, profile_fact_record, recall_chunk_record, typed_memory_record,
};
pub use provider::{MemoryProvider, StoreMemoryProvider};
pub use resource::MemoryResourceHandler;
#[cfg(test)]
pub(crate) use state_capture::STATE_CAPTURE_PROVENANCE_LABEL;
pub use state_capture::personal_assistant_state_capture_proposals;
pub(crate) use util::encode_memory_segment;
