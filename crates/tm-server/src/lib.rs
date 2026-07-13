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
pub mod native_deno;
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
    DreamModelRoles, DreamWorkerConfig, DreamWorkerDaemon, DreamWorkerDaemonHandle,
    ServerDreamWorker,
};
pub use error::{Result, ServerError};
pub use memory::{
    MemoryContext, MemoryProvider, MemoryRecordRef, MemoryWriteKind, MemoryWriteProposal,
    MemoryWriteStatus, ProfileFactProposalInput, StoreMemoryProvider,
};
pub use native_deno::{
    HttpApprovalPolicy, NativeApprovalMode, NativeDenoBackend, NativeDenoBackendOptions,
};
pub use omp_acp::{OmpAcpBackend, OmpAcpConfig};
pub use push::{
    FakePushProvider, InMemoryPushStore, PostgresPushStore, PushCipher, PushMessage,
    PushMessageKind, PushProvider, PushProviderOutcome, PushProviderResult,
    PushRegistrationMetadata, PushRuntimeMetrics, PushService, PushStore,
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
    EpisodicMemoryStore, MemoryError, MemoryEvidenceRef, MemoryStoreError, MemoryStoreResult,
    MemorySummaryKind, MemorySummaryRecord, MemorySummaryStore, NewDreamQueueRecord,
    NewMemorySummaryRecord, NewSkillProposalRecord, NoopDreamWorker, ProfileFactRecord,
    ProfileMemoryStore, RecallChunkRecord, SkillProposalRecord, SkillProposalStatus,
    SkillProposalStore, SkillVerification,
};
pub use tm_modes::{
    AssetStatus, ComposedPrompt, ModeAssets, ModeCatalog, ModeId, ModeProfile, ModesConfig,
    ReviewAddendumChange, ReviewAddendumSection, ReviewApplyContract, ReviewMetadata,
    ReviewProposalStatus, ReviewProposalTarget,
};
