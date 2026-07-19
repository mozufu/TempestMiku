use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

use crate::actor::{ActorDigest, ActorSpec};
use crate::supervise::FailureReason;

/// Errors that can occur during actor execution (P3.1).
#[derive(Debug, Error)]
pub enum ActorError {
    #[error("actor execution failed: {0}")]
    Execution(String),
    #[error("actor spec invalid: {0}")]
    InvalidSpec(String),
    #[error("actor depth limit {0} exceeded")]
    DepthExceeded(u32),
    #[error("actor wall-clock budget exceeded")]
    Timeout,
    #[error("actor cancelled")]
    Cancelled,
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

pub fn failure_reason_for_error(err: &ActorError) -> FailureReason {
    match err {
        ActorError::Execution(msg) => FailureReason::Crash {
            message: msg.clone(),
        },
        ActorError::InvalidSpec(msg) => FailureReason::Crash {
            message: msg.clone(),
        },
        ActorError::DepthExceeded(_) => FailureReason::DepthExceeded,
        ActorError::Timeout => FailureReason::Timeout,
        ActorError::Cancelled => FailureReason::Cancelled,
    }
}

pub async fn run_to_digest_with_budget(
    executor: Arc<dyn ActorExecutor>,
    spec: ActorSpec,
) -> Result<ActorDigest, ActorError> {
    spec.validate_text_limits()
        .map_err(ActorError::InvalidSpec)?;
    let timeout = Duration::from_millis(spec.budget.wall_ms);
    let deadline = tokio::time::Instant::now() + timeout;
    let cancellation = spec.cancellation.clone();
    match tokio::time::timeout_at(deadline, executor.run_to_digest(spec)).await {
        Ok(Ok(mut digest)) => {
            // `tokio::time::timeout` polls the inner future before checking the
            // timer. If a busy runtime wakes both after the deadline, a late
            // actor can therefore appear successful unless completion is
            // fenced against the same absolute deadline here.
            if tokio::time::Instant::now() >= deadline {
                cancellation.cancel();
                return Err(ActorError::Timeout);
            }
            digest.enforce_text_limits();
            Ok(digest)
        }
        Ok(Err(error)) => Err(error),
        Err(_) => {
            cancellation.cancel();
            Err(ActorError::Timeout)
        }
    }
}
