use async_trait::async_trait;
use tm_host::InvocationCtx;

use crate::{McpError, Result};

/// Explicit host-owned authority used only while staging an MCP catalog.
///
/// Catalog discovery is an operator action, not a model invocation. Requiring this wrapper keeps
/// discovery from accidentally borrowing the grants, actor identity, approval channel, or audit
/// sink of whichever turn happened to trigger a reload.
#[derive(Clone)]
pub struct McpCatalogContext {
    invocation: InvocationCtx,
}

impl std::fmt::Debug for McpCatalogContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpCatalogContext")
            .field("invocation", &self.invocation)
            .finish()
    }
}

impl McpCatalogContext {
    /// Wrap a dedicated host context. It must have a stable session id and cannot impersonate an
    /// actor; the configured grants and event sink remain authoritative for discovery traffic.
    pub fn new(invocation: InvocationCtx) -> Result<Self> {
        if invocation.session_id.trim().is_empty() {
            return Err(McpError::InvalidConfig(
                "MCP catalog context requires a non-empty host session id".to_string(),
            ));
        }
        if invocation.actor_id.is_some() {
            return Err(McpError::InvalidConfig(
                "MCP catalog context must be host-owned, not actor-bound".to_string(),
            ));
        }
        Ok(Self { invocation })
    }

    pub(crate) fn invocation(&self) -> &InvocationCtx {
        &self.invocation
    }
}

/// Transport seam for a single JSON-RPC exchange.
///
/// `server` is a validated local alias, not a destination URL. Production implementations are
/// expected to resolve it through trusted configuration and the P9 egress/secret broker.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Exchange one encoded JSON-RPC request/response. The core checks request bytes before this
    /// call and response bytes before JSON parsing; production transports must additionally stop
    /// reading at the P9 response cap rather than buffering an unbounded peer body.
    async fn request(&self, ctx: &InvocationCtx, server: &str, request: &[u8]) -> Result<Vec<u8>>;

    async fn notify(&self, ctx: &InvocationCtx, server: &str, notification: &[u8]) -> Result<()>;
}
