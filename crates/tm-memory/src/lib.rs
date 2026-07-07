//! TempestMiku memory ownership crate.
//!
//! The first P4 slice is intentionally narrow: own the durable dream-queue
//! record shape and provide a no-op worker contract. Extraction, reflection,
//! summaries, embeddings, and skill proposals attach here in later P4 slices.

mod dream;

pub use dream::{
    DreamQueueRecord, DreamReason, DreamStatus, DreamWorker, DreamWorkerReport, MemoryError,
    NewDreamQueueRecord, NoopDreamWorker,
};
