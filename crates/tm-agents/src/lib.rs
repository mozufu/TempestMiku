//! TempestMiku actor lifecycle, mailbox, orchestration, supervision, and agent:// resources.
//!
//! P3 implementation — `agents.run`, `agents.spawn`, and `agents.parallel` are live when an
//! `ActorExecutor` is injected at startup. `agents.msg` is registered and grant-checked, but its
//! concrete mailbox request/reply behavior is still pending.
//!
//! Crate layout (§23.8):
//! - [`actor`]      — identity, lifecycle, status, handle, budget, digest, lifecycle events
//! - [`executor`]   — `ActorExecutor` trait injected by `tm-server` at startup
//! - [`mailbox`]    — message types, actor roster registry (`MailboxRegistry`)
//! - [`orchestrate`] — P3 MVP `agents.*` HostFns (run/spawn/parallel live, msg pending); `register`
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
