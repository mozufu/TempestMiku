//! TempestMiku memory ownership crate.
//!
//! Owns durable dream, summary, skill, record, evidence, recall-evaluation, and embedding-provenance
//! contracts. Concrete Postgres and in-memory persistence remains in `tm-server`.

mod dialectic;
mod dream;
mod durable;
mod embedding;
mod evaluation;
mod evolution;
mod hybrid;
mod input;
mod records;
mod redaction;
mod skill;
mod store;
mod summary;

pub use dialectic::{
    DEFAULT_DIALECTIC_CADENCE, DEFAULT_DIALECTIC_MAX_CHARS, DIALECTIC_EVENT_TYPE,
    DIALECTIC_SCHEMA_VERSION, DIALECTIC_SYSTEM_PROMPT, DialecticFact, DialecticRequest,
    DialecticStatus, DialecticTrace, MAX_DIALECTIC_FACT_CHARS, MAX_DIALECTIC_FACTS,
    MAX_DIALECTIC_QUERY_CHARS, MAX_DIALECTIC_SOURCE_URI_CHARS,
};
pub use dream::{
    DreamLease, DreamQueueRecord, DreamReason, DreamStatus, DreamWorker, DreamWorkerReport,
    MemoryError, NewDreamQueueRecord, NoopDreamWorker,
};
pub use durable::{
    DurableMemoryReadiness, DurableMemoryRecordError, EmbeddingReadiness,
    MAX_PGVECTOR_INDEX_DIMENSIONS, MemoryEmbeddingGeneration, MemoryEmbeddingGenerationStatus,
    MemoryEmbeddingJobClaim, MemoryEmbeddingJobLease, MemoryEmbeddingJobRecord,
    MemoryEmbeddingJobStatus, MemorySchemaReadiness, MemoryScopeTombstone,
    NewMemoryEmbeddingGeneration, NewMemoryEmbeddingJob, PgVectorReadiness, StoredMemoryRecord,
};
pub use embedding::{
    DEFAULT_EMBEDDING_MAX_BATCH_SIZE, DEFAULT_EMBEDDING_MAX_INPUT_BYTES,
    DEFAULT_EMBEDDING_TIMEOUT_MS, EMBEDDING_PROVENANCE_SCHEMA_VERSION, EmbeddingClient,
    EmbeddingConfig, EmbeddingConfigError, EmbeddingError, EmbeddingInput, EmbeddingNormalization,
    EmbeddingProvenance, EmbeddingProvenanceError, EmbeddingProvider, EmbeddingRequest,
    EmbeddingResponse, EmbeddingVector, MAX_EMBEDDING_BATCH_SIZE, MAX_EMBEDDING_INPUT_BYTES,
    ReembeddingState, embedding_content_hash, embedding_text,
};
pub use evaluation::{
    RECALL_EVALUATION_SCHEMA_VERSION, RECALL_EVALUATOR_VERSION, RecallAcceptanceCohort,
    RecallAcceptancePolicy, RecallBaselineArtifact, RecallBaselineEnvironment,
    RecallDeterministicAggregate, RecallDeterministicCaseReport, RecallDeterministicReport,
    RecallDeterministicSplitReport, RecallEvaluationAggregate, RecallEvaluationCase,
    RecallEvaluationCaseReport, RecallEvaluationError, RecallEvaluationManifest,
    RecallEvaluationObservation, RecallEvaluationReport, RecallEvaluationSplit,
    RecallEvaluationSplitReport, RecallFalseInclusionCounts, RecallFalseInclusionKind,
    RecallFixtureCoverage, RecallFixtureRecord, RecallRecordQuality, RecallRelevanceJudgment,
    evaluate_recall_observations,
};
pub use evolution::{
    EnvironmentCognitionRecord, EpisodeStatus, EvolutionEpisodeRecord, EvolutionPolicyRecord,
    ExperienceTraceRecord, FeedbackOutcome, NewEvolutionEpisodeRecord, NewExperienceTraceRecord,
    PolicyStatus, RewardSource, TraceKind, UnknownEpisodeStatus, UnknownFeedbackOutcome,
    UnknownPolicyStatus, UnknownRewardSource, UnknownTraceKind, backfill_trace_values,
    error_signature, policy_gain, skill_reliability,
};
pub use hybrid::{
    DEFAULT_HYBRID_CANDIDATE_LIMIT, DEFAULT_HYBRID_TOP_K, DEFAULT_RRF_K, DenseRecallQuery,
    DenseRecallStatus, HybridMemoryCandidate, HybridRecallError, HybridRecallRequest,
    HybridRecallResult, MAX_HYBRID_CANDIDATE_LIMIT, RankedMemoryCandidate, fuse_hybrid_candidates,
};
pub use input::{BudgetedDreamInput, DreamInputBudget, DreamInputChunk, DreamInputMessage};
pub use records::{
    EpisodicMemoryRecord, MEMORY_RECORD_SCHEMA_VERSION, MemoryEvidenceSource,
    MemoryRecordContractError, MemoryRecordEvidence, MemoryRecordKind, MemoryRecordLinks,
    MemoryRecordResource, MemoryRecordStatus, ProfileFactRecord, RecallChunkRecord,
    SemanticMemoryRecord,
};
pub use redaction::{
    Redaction, RedactionReport, contains_sensitive_data, redact_dream_text, redact_json_value,
};
pub use skill::{
    MAX_SKILL_PROPOSAL_BODY_BYTES, MAX_SKILL_PROPOSAL_REFERENCES, NewSkillProposalRecord,
    SkillCatalogReloadContract, SkillConflictPolicy, SkillProposalLifecycle, SkillProposalRecord,
    SkillProposalStatus, SkillRollbackContract, SkillVerification, UnknownSkillProposalStatus,
    new_skill_proposal_lifecycle, skill_proposal_lifecycle,
};
pub use store::{
    DreamLeaseStore, EpisodicMemoryStore, MemoryStoreError, MemoryStoreResult, MemorySummaryStore,
    ProfileMemoryStore, SkillProposalStore,
};
pub use summary::{
    MemoryEvidenceRef, MemorySummaryKind, MemorySummaryRecord, NewMemorySummaryRecord,
    UnknownMemorySummaryKind,
};
