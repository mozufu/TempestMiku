//! TempestMiku P0 server slice.
//!
//! This crate exposes a small `axum` session API plus replayable SSE events. The storage
//! traits are Postgres-shaped, while tests use the in-memory implementation so normal
//! `cargo test` needs no external service.

pub mod api;
pub mod auth;
pub mod chat;
pub mod coding_backend;
pub mod dream;
pub mod error;
mod evolution;
pub mod memory;
pub mod native_tm;
pub mod omp_acp;
pub mod push;
pub mod runtime;
pub mod scheduler;
pub mod store;
pub mod webui;

pub use api::{AppState, app};
pub use auth::{
    AuthConfig, AuthContext, AuthDeviceRecord, AuthDeviceStore, AuthPrincipal, DeviceAuthConfig,
    DeviceCredential, ForwardedAuthConfig, InMemoryAuthDeviceStore, NewAuthDevice, NewPairingCode,
    PairingCodeRecord,
};
pub use chat::{
    AgentChatRunner, AgentChatRunnerOptions, ChatActorExecutor, ChatRunLimits, ChatRunner,
    ChatTurn, EchoChatRunner, PersistingEventSink, RosterCodingEventSink, ServerChatRunner,
};
pub use coding_backend::{
    ApprovalBroker, ApprovalOption, ApprovalOutcome, ApprovalPrompt, ApprovalResolveDecision,
    ApprovalStatus, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult,
    DetailedApprovalOutcome, DurableApprovalSpec, ResolveApprovalRequest, StoreCodingEventSink,
};
pub use dream::{
    DreamModelRoles, DreamWorkerConfig, DreamWorkerDaemon, DreamWorkerDaemonHandle, SenderFactory,
    ServerDreamWorker,
};
pub use error::{Result, ServerError};
pub use memory::{
    IN_MEMORY_LEXICAL_BASELINE_MODE, LocalEmbeddingHttpClient, MemoryCandidateTrace, MemoryContext,
    MemoryEmbeddingWorker, MemoryProvider, MemoryRecordRef, MemoryRetrievalMode,
    MemoryRetrievalTrace, MemoryWriteKind, MemoryWriteProposal, MemoryWriteStatus,
    POSTGRES_HYBRID_RECALL_MODE, POSTGRES_LEXICAL_BASELINE_MODE, PostgresMemoryEmbeddingWorker,
    ProfileFactProposalInput, StoreMemoryProvider, evaluate_lexical_recall_baseline,
    evaluate_memory_provider_recall,
};
pub use native_tm::{
    HttpApprovalPolicy, NativeApprovalMode, NativeTmBackend, NativeTmBackendOptions,
};
pub use omp_acp::{OmpAcpBackend, OmpAcpConfig};
pub use push::{
    FakePushProvider, InMemoryPushStore, PostgresPushStore, PushCipher, PushMessage,
    PushMessageKind, PushProvider, PushProviderOutcome, PushProviderResult,
    PushRegistrationMetadata, PushRuntimeMetrics, PushService, PushStore, UnifiedPushProvider,
};
pub use runtime::{RuntimeConfig, RuntimeStatus, RuntimeStatusSnapshot, ServerRole, run_server};
pub use scheduler::{
    CronSchedule, SchedulerBounds, WEEKLY_SHIP_LEDGER_JOB_ID, WEEKLY_SHIP_LEDGER_SCHEDULE,
    ensure_weekly_ship_ledger_job, trigger_weekly_ship_ledger, weekly_ship_ledger_job,
};
pub use store::{
    ApprovalEffectLease, ApprovalEffectRecord, ApprovalRequestRecord, CronJobRecord, CronLease,
    CronRunRecord, EndSessionDreamResult, EvolutionAuditEntry, EvolutionReviewProposalRecord,
    InMemoryStore, MessageRecord, ModeState, NewApprovalRequest, NewApprovalResolution,
    NewCronJobRecord, NewCronRunRecord, NewEvolutionReviewProposal, NewProjectItem, NewSession,
    PostgresDriveMetadataStore, PostgresStore, ProjectItemKind, ProjectItemRecord, SessionEvent,
    SessionRecord, SessionSummaryRecord, SessionTurnRecord, Store, StoreEvent, StoreRuntimeMetrics,
};
pub use tm_agents::MailboxRegistry;
pub use tm_memory::{
    BudgetedDreamInput, DreamInputBudget, DreamInputChunk, DreamInputMessage, DreamLease,
    DreamLeaseStore, DreamQueueRecord, DreamReason, DreamStatus, DreamWorker, DreamWorkerReport,
    EmbeddingNormalization, EmbeddingProvenance, EmbeddingProvenanceError, EpisodicMemoryRecord,
    EpisodicMemoryStore, MemoryError, MemoryEvidenceRef, MemoryRecordContractError,
    MemoryRecordEvidence, MemoryRecordLinks, MemoryRecordResource, MemoryRecordStatus,
    MemoryStoreError, MemoryStoreResult, MemorySummaryKind, MemorySummaryRecord,
    MemorySummaryStore, NewDreamQueueRecord, NewMemorySummaryRecord, NewSkillProposalRecord,
    NoopDreamWorker, ProfileFactRecord, ProfileMemoryStore, RecallBaselineArtifact,
    RecallChunkRecord, RecallEvaluationManifest, RecallEvaluationReport, ReembeddingState,
    SemanticMemoryRecord, SkillProposalRecord, SkillProposalStatus, SkillProposalStore,
    SkillVerification,
};
pub use tm_modes::{
    AssetStatus, ComposedPrompt, ModeAssets, ModeCatalog, ModeId, ModeProfile, ModesConfig,
    ReviewAddendumChange, ReviewAddendumSection, ReviewApplyContract, ReviewMetadata,
    ReviewProposalStatus, ReviewProposalTarget,
};
