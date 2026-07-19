use std::sync::Arc;

use async_trait::async_trait;

use super::Store;

/// Adapts the server's durable Store to tm-mcp without giving the protocol crate any dependency on
/// server persistence. Both begin and finish retain the Store's atomic insert/CAS semantics.
pub struct StoreMcpMutationEffectStore<S> {
    store: Arc<S>,
}

impl<S> StoreMcpMutationEffectStore<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S> tm_mcp::McpMutationEffectStore for StoreMcpMutationEffectStore<S>
where
    S: Store,
{
    async fn begin(
        &self,
        intent: tm_mcp::McpMutationIntent,
    ) -> tm_mcp::Result<tm_mcp::McpMutationEffectClaim> {
        self.store
            .begin_mcp_mutation_effect(intent)
            .await
            .map_err(store_error)
    }

    async fn finish(
        &self,
        effect_id: &str,
        status: tm_mcp::McpMutationEffectStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> tm_mcp::Result<tm_mcp::McpMutationEffectRecord> {
        self.store
            .finish_mcp_mutation_effect(
                effect_id,
                status,
                result_digest,
                result_bytes,
                error_code,
                error_digest,
            )
            .await
            .map_err(store_error)
    }
}

fn store_error(error: crate::ServerError) -> tm_mcp::McpError {
    let code = match error {
        crate::ServerError::NotFound(_) => "not_found",
        crate::ServerError::Unauthorized => "unauthorized",
        crate::ServerError::Forbidden | crate::ServerError::Policy(_) => "policy_denied",
        crate::ServerError::InvalidRequest(_) => "invalid_request",
        crate::ServerError::Conflict(_) => "conflict",
        crate::ServerError::Store(_) => "store_error",
        crate::ServerError::Backend(_) => "backend_error",
    };
    tm_mcp::McpError::Unavailable(format!("durable MCP mutation effect store failed ({code})"))
}
