//! TempestMiku actor lifecycle, mailbox, orchestration, supervision, and agent:// resources.
//!
//! P3/P3-plus implementation — `agents.run`, `agents.spawn`, `agents.parallel`, `agents.msg`,
//! and the live mailbox primitives (`agents.send`, `agents.broadcast`, `agents.wait`,
//! `agents.inbox`, `agents.list`, `agents.cancel`) are live when an `ActorExecutor`
//! is injected at startup.
//!
//! Crate layout (§23.8):
//! - [`actor`]      — identity, lifecycle, status, handle, budget, digest, lifecycle events
//! - [`executor`]   — `ActorExecutor` trait injected by `tm-server` at startup
//! - [`mailbox`]    — message types, bounded actor inboxes, actor roster registry (`MailboxRegistry`)
//! - [`orchestrate`] — P3/P3-plus `agents.*` HostFns; `register`
//! - [`supervise`]  — supervision tree types, restart strategies, failure reasons
//! - [`resources`]  — `agent://` and `history://` ResourceHandlers

pub mod actor;
pub mod executor;
pub mod mailbox;
pub mod orchestrate;
pub mod resources;
pub mod supervise;

pub use actor::{
    ActorBudget, ActorCancelToken, ActorDigest, ActorHandle, ActorId, ActorIdError,
    ActorLifecycleEvent, ActorRecord, ActorSpec, ActorStatus,
};
pub use executor::{ActorError, ActorExecutor};
pub use mailbox::{ActorMessage, MailboxRegistry, Receipt};
pub use orchestrate::{caps, register};
pub use resources::{AgentResourceHandler, HistoryResourceHandler};
pub use supervise::{
    FailureReason, RestartStrategy, SupervisionAction, SupervisionDecision, SupervisionPolicy,
    Supervisor,
};
