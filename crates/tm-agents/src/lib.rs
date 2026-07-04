//! TempestMiku actor lifecycle, mailbox, orchestration, supervision, and agent:// resources.
//!
//! P3 implementation — `agents.run` is live when an `ActorExecutor` is injected at startup.
//! `agents.spawn`, `agents.parallel`, and `agents.msg` are capability-gated stubs deferred
//! to P3.2 when their mailbox channel layer has concrete callers.
//!
//! Crate layout (§23.8):
//! - [`actor`]      — identity, lifecycle, status, handle, budget, digest, lifecycle events
//! - [`executor`]   — `ActorExecutor` trait injected by `tm-server` at startup
//! - [`mailbox`]    — message types, actor roster registry (`MailboxRegistry`)
//! - [`orchestrate`] — `agents.*` HostFns (run live, spawn/parallel/msg stubbed); `register`
//! - [`supervise`]  — supervision tree types, restart strategies, failure reasons
//! - [`resources`]  — `agent://` (live reads/list) and `history://` (stub) ResourceHandlers

pub mod actor;
pub mod executor;
pub mod mailbox;
pub mod orchestrate;
pub mod resources;
pub mod supervise;

pub use actor::{
    ActorBudget, ActorDigest, ActorHandle, ActorId, ActorIdError, ActorLifecycleEvent, ActorRecord,
    ActorSpec, ActorStatus,
};
pub use executor::{ActorError, ActorExecutor};
pub use mailbox::{ActorMessage, MailboxRegistry, Receipt};
pub use orchestrate::{caps, register};
pub use resources::{AgentResourceHandler, HistoryResourceHandler};
pub use supervise::{FailureReason, RestartStrategy, SupervisionPolicy, Supervisor};
