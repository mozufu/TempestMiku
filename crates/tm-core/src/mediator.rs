use async_trait::async_trait;
use serde_json::Value;

use crate::{Result, ToolSpec};

/// A seam for exactly one product-layer tool call per agent turn, kept mode-agnostic here.
///
/// `tm-core`'s only native tool is `execute` (design §5.2); everything else the model might
/// need to call is delegated through this trait so the loop never has to learn what the
/// product layer's tools actually mean. A [`crate::Agent`] configured with a mediator
/// advertises the mediator's [`ToolMediator::tool_specs`] alongside `execute`; when the model
/// calls one of them, the loop hands the call to [`ToolMediator::handle`] and feeds the
/// returned string back as that call's tool result, then continues the turn loop exactly as
/// it would after an `execute` call.
///
/// `handle` is async so implementations can await a real round-trip (e.g. a server-side
/// approval broker) directly on whatever runtime the agent loop is already driven from,
/// without a sync-to-async forwarder. Errors are not fatal to the turn: the loop turns an
/// `Err` into a plain-text tool result so one failed mediated call never aborts the
/// conversation.
#[async_trait]
pub trait ToolMediator: Send + Sync {
    /// Tool specs advertised to the model alongside `execute`. Empty means "no extra tools
    /// this turn" (e.g. a mode where the mediated action isn't offered).
    fn tool_specs(&self) -> Vec<ToolSpec>;

    /// Handle a non-`execute` tool call and return the content for its tool result message.
    async fn handle(&self, name: &str, arguments: &Value) -> Result<String>;
}
