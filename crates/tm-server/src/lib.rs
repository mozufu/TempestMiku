//! TempestMiku P0 server slice.
//!
//! This crate exposes a small `axum` session API plus replayable SSE events. The storage
//! traits are Postgres-shaped, while tests use the in-memory implementation so normal
//! `cargo test` needs no external service.

pub mod api;
pub mod auth;
pub mod chat;
pub mod error;
pub mod memory;
pub mod persona;
pub mod store;
pub mod webui;

pub use api::{AppState, app};
pub use auth::{AuthConfig, ForwardedAuthConfig};
pub use chat::{AgentChatRunner, ChatRunner, EchoChatRunner, PersistingEventSink};
pub use error::{Result, ServerError};
pub use memory::{MemoryContext, MemoryProvider, StoreMemoryProvider};
pub use persona::{Mode, PersonaConfig, PersonaStatus};
pub use store::{
    InMemoryStore, MessageRecord, NewSession, PostgresStore, SessionEvent, SessionRecord, Store,
    StoreEvent,
};
