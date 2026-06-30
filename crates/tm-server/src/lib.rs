//! TempestMiku P0 server slice.
//!
//! This crate exposes a small `axum` session API plus replayable SSE events. The storage
//! traits are Postgres-shaped, while tests use the in-memory implementation so normal
//! `cargo test` needs no external service.

pub mod api;
pub mod auth;
pub mod chat;
pub mod coding_backend;
pub mod error;
pub mod memory;
pub mod omp_acp;
pub mod store;
pub mod webui;

pub use api::{AppState, app};
pub use auth::{AuthConfig, ForwardedAuthConfig};
pub use chat::{
    AgentChatRunner, ChatRunner, EchoChatRunner, PersistingEventSink, ServerChatRunner,
};
pub use coding_backend::{
    ApprovalBroker, ApprovalOption, ApprovalOutcome, ApprovalPrompt, ApprovalResolveDecision,
    CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult, ResolveApprovalRequest,
    StoreCodingEventSink,
};
pub use error::{Result, ServerError};
pub use memory::{MemoryContext, MemoryProvider, StoreMemoryProvider};
pub use omp_acp::{OmpAcpBackend, OmpAcpConfig};
pub use store::{
    InMemoryStore, MessageRecord, ModeState, NewProjectItem, NewSession, PostgresStore,
    ProjectItemKind, ProjectItemRecord, SessionEvent, SessionRecord, Store, StoreEvent,
};
pub use tm_persona::{Mode, PersonaConfig, PersonaStatus};
