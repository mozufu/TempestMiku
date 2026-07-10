//! TempestMiku memory ownership crate.
//!
//! The first P4 slice is intentionally narrow: own the durable dream-queue
//! record shape and provide a no-op worker contract. Extraction, reflection,
//! summaries, embeddings, and skill proposals attach here in later P4 slices.

mod dream;
mod embedding;
mod input;
mod records;
mod redaction;
mod skill;
mod store;
mod summary;

pub use dream::{
    DreamLease, DreamQueueRecord, DreamReason, DreamStatus, DreamWorker, DreamWorkerReport,
    MemoryError, NewDreamQueueRecord, NoopDreamWorker,
};
pub use embedding::{EmbeddingConfig, EmbeddingConfigError, EmbeddingProvider};
pub use input::{BudgetedDreamInput, DreamInputBudget, DreamInputChunk, DreamInputMessage};
pub use records::{ProfileFactRecord, RecallChunkRecord};
pub use redaction::{
    Redaction, RedactionReport, contains_sensitive_data, redact_dream_text, redact_json_value,
};
pub use skill::{
    NewSkillProposalRecord, SkillProposalRecord, SkillProposalStatus, SkillVerification,
    UnknownSkillProposalStatus,
};
pub use store::{
    DreamLeaseStore, EpisodicMemoryStore, MemoryStoreError, MemoryStoreResult, MemorySummaryStore,
    ProfileMemoryStore, SkillProposalStore,
};
pub use summary::{
    MemoryEvidenceRef, MemorySummaryKind, MemorySummaryRecord, NewMemorySummaryRecord,
    UnknownMemorySummaryKind,
};
