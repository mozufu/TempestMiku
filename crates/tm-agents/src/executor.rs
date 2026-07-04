use async_trait::async_trait;
use thiserror::Error;

use crate::actor::{ActorDigest, ActorSpec};

/// Errors that can occur during actor execution (P3.1).
#[derive(Debug, Error)]
pub enum ActorError {
    #[error("actor execution failed: {0}")]
    Execution(String),
    #[error("actor spec invalid: {0}")]
    InvalidSpec(String),
    #[error("actor depth limit {0} exceeded")]
    DepthExceeded(u32),
}

/// Runs an actor spec to completion and returns a bounded digest (§23.3, P3.2).
///
/// Implemented by `tm-server`'s `ChatActorExecutor` using the existing agent loop.
/// `tm-agents` defines the trait; `tm-server` provides the impl and injects it at startup
/// via [`crate::mailbox::MailboxRegistry::set_executor`].
#[async_trait]
pub trait ActorExecutor: Send + Sync + 'static {
    async fn run_to_digest(&self, spec: ActorSpec) -> Result<ActorDigest, ActorError>;
}
