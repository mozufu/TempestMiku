use std::sync::Arc;

use async_trait::async_trait;
use tm_core::{EventSink, Message};
use tm_host::HostFn;
use tm_modes::ModeId;
use uuid::Uuid;

use crate::Result;

#[derive(Clone)]
pub struct ChatTurn {
    pub session_id: Uuid,
    /// Durable queue identity. `Some` keeps successful runtime state quarantined until the
    /// dispatcher confirms the matching store commit; non-durable callers use `None`.
    pub durable_turn_id: Option<Uuid>,
    pub user_prompt: String,
    pub mode: ModeId,
    pub scope: String,
    pub system_prompt: String,
    pub capabilities: Vec<String>,
    pub prior_messages: Vec<Message>,
    pub limits: ChatRunLimits,
    pub deny_approvals: bool,
    /// Server-owned SDK handlers installed for this turn. Registration does not grant
    /// authority; `capabilities` remains the exact grant set enforced by the sandbox.
    pub host_functions: Vec<Arc<dyn HostFn>>,
}

impl std::fmt::Debug for ChatTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatTurn")
            .field("session_id", &self.session_id)
            .field("durable_turn_id", &self.durable_turn_id)
            .field("user_prompt", &self.user_prompt)
            .field("mode", &self.mode)
            .field("scope", &self.scope)
            .field("system_prompt", &self.system_prompt)
            .field("capabilities", &self.capabilities)
            .field("prior_messages", &self.prior_messages)
            .field("limits", &self.limits)
            .field("deny_approvals", &self.deny_approvals)
            .field(
                "host_functions",
                &self
                    .host_functions
                    .iter()
                    .map(|function| function.name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatRunLimits {
    pub max_turns: Option<usize>,
    pub cell_wall_ms: Option<u64>,
}

#[async_trait]
pub trait ChatRunner: Send + Sync + 'static {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String>;

    /// Promote a successful turn's pending runtime state after its durable turn commit succeeds.
    async fn promote_session(&self, _session_id: Uuid, _turn_id: Uuid) -> Result<()> {
        Ok(())
    }

    /// Discard pending or previously promoted ephemeral state after a durable turn aborts.
    async fn abort_session(&self, _session_id: Uuid, _turn_id: Uuid) -> Result<()> {
        Ok(())
    }

    /// Request cooperative termination of every active/queued turn for this session.
    fn cancel_turn(&self, _session_id: Uuid, _turn_id: Uuid) {}
}

#[async_trait]
impl<T: ChatRunner + ?Sized> ChatRunner for Arc<T> {
    async fn run_turn(
        &self,
        turn: ChatTurn,
        sink: Arc<dyn EventSink + Send + Sync>,
    ) -> Result<String> {
        (**self).run_turn(turn, sink).await
    }

    async fn promote_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        (**self).promote_session(session_id, turn_id).await
    }

    async fn abort_session(&self, session_id: Uuid, turn_id: Uuid) -> Result<()> {
        (**self).abort_session(session_id, turn_id).await
    }

    fn cancel_turn(&self, session_id: Uuid, turn_id: Uuid) {
        (**self).cancel_turn(session_id, turn_id);
    }
}
