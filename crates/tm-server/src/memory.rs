mod context;
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

pub use context::{MemoryContext, MemoryPromptBudget, MemoryPromptItem};
pub use proposal::{
    MemoryRecordRef, MemoryWriteKind, MemoryWriteProposal, MemoryWriteStatus, profile_fact_record,
    recall_chunk_record,
};
pub use provider::{MemoryProvider, StoreMemoryProvider};
pub use resource::MemoryResourceHandler;
#[cfg(test)]
pub(crate) use state_capture::STATE_CAPTURE_PROVENANCE_LABEL;
pub use state_capture::personal_assistant_state_capture_proposals;
pub(crate) use util::encode_memory_segment;
