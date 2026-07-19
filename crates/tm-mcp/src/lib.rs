//! Bounded import of selected MCP 2025-11-25 capabilities.
//!
//! The protocol/catalog layer remains transport-abstract. Its production Streamable HTTP adapter
//! routes every request through the P9 egress/opaque-secret boundary, so this crate never gains an
//! ambient socket, DNS, or credential path.

mod bindings;
mod catalog;
mod effects;
mod error;
mod http_transport;
mod transport;
mod types;
mod validate;

pub use bindings::{McpBindings, McpResourceHandler};
pub use catalog::McpCatalogManager;
pub use effects::{
    McpMutationEffectClaim, McpMutationEffectRecord, McpMutationEffectStatus,
    McpMutationEffectStore, McpMutationIntent, VolatileMcpMutationEffectStore,
};
pub use error::{McpError, Result};
pub use http_transport::{EgressMcpTransport, McpHttpServerConfig, McpHttpTransportBounds};
pub use transport::{McpCatalogContext, McpTransport};
pub use types::*;

/// The only MCP protocol revision accepted by this import layer.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

#[cfg(test)]
mod http_transport_tests;
#[cfg(test)]
mod live_canary_tests;
#[cfg(test)]
mod tests;
