use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_host::ResourceHandler;
use uuid::Uuid;

use crate::{Result, SessionEvent};

#[async_trait]
pub trait CodingBackend: Send + Sync + 'static {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult>;

    /// Promote quarantined runtime state after the matching durable turn commits.
    async fn promote_session(&self, _session_id: Uuid, _turn_id: Uuid) -> Result<()> {
        Ok(())
    }

    /// Cancel and evict state produced by an uncommitted durable turn.
    async fn abort_session(&self, _session_id: Uuid, _turn_id: Uuid) -> Result<()> {
        Ok(())
    }

    /// Request cooperative cancellation without waiting for backend cleanup.
    fn cancel_turn(&self, _session_id: Uuid, _turn_id: Uuid) {}
}

#[derive(Clone)]
pub struct CodingTurn {
    pub session_id: Uuid,
    /// Durable queue identity. Native tm keeps successful runtime state quarantined until this
    /// exact turn is committed; standalone/non-durable callers use `None`.
    pub durable_turn_id: Option<Uuid>,
    pub user_prompt: String,
    pub system_prompt: String,
    pub mode: tm_modes::ModeId,
    /// Project authority drives sandbox fs/proc access; memory scope drives memory.search.
    pub owner_subject: String,
    pub project_id: Option<String>,
    pub memory_scope: String,
    /// Exact capabilities declared for this turn (e.g. `["agents.*", "backend.coding"]`).
    /// The sandbox replaces its externally authorized grants with this set; `.*` capability
    /// patterns remain supported. Runtime-intrinsic artifact output and catalog inspection do not
    /// grant host, resource-read, network, or child authority.
    pub capabilities: Vec<String>,
    /// Caller-bounded persisted conversation history, ordered oldest to newest.
    pub prior_messages: Vec<tm_core::Message>,
    /// Server-owned resource handlers installed for this turn. Registration does not grant
    /// authority; `capabilities` remains the exact grant set enforced by the sandbox.
    pub resource_handlers: Vec<Arc<dyn ResourceHandler>>,
}

impl std::fmt::Debug for CodingTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodingTurn")
            .field("session_id", &self.session_id)
            .field("durable_turn_id", &self.durable_turn_id)
            .field("user_prompt", &self.user_prompt)
            .field("system_prompt", &self.system_prompt)
            .field("mode", &self.mode)
            .field("owner_subject", &self.owner_subject)
            .field("project_id", &self.project_id)
            .field("memory_scope", &self.memory_scope)
            .field("capabilities", &self.capabilities)
            .field("prior_messages", &self.prior_messages)
            .field(
                "resource_handlers",
                &self
                    .resource_handlers
                    .iter()
                    .map(|handler| handler.scheme())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodingTurnResult {
    pub final_text: String,
    pub transcript_artifact: Option<tm_artifacts::ArtifactRef>,
}

#[async_trait]
pub trait CodingEventSink: Send + Sync + 'static {
    async fn emit(&self, event_type: &str, payload_json: Value) -> Result<SessionEvent>;

    async fn publish_persisted(&self, _event: SessionEvent) -> Result<()> {
        Ok(())
    }

    fn turn_id(&self) -> Option<Uuid> {
        None
    }
}
