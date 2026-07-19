use thiserror::Error;

pub type Result<T, E = McpError> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum McpError {
    #[error("invalid MCP configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid MCP data from {server}: {message}")]
    InvalidRemote { server: String, message: String },
    #[error("MCP bounds exceeded for {target}: {limit}")]
    Bounds { target: String, limit: String },
    #[error("MCP protocol negotiation failed for {server}: expected {expected}, got {actual}")]
    ProtocolVersion {
        server: String,
        expected: String,
        actual: String,
    },
    #[error("MCP server {server} did not negotiate required capability {capability}")]
    MissingCapability { server: String, capability: String },
    #[error("MCP allowlisted object missing from {server}: {kind} {name}")]
    MissingAllowlistedObject {
        server: String,
        kind: String,
        name: String,
    },
    #[error("MCP catalog collision: {0}")]
    Collision(String),
    #[error("MCP JSON-RPC error from {server} for {method}: code {code}, digest {digest}")]
    Rpc {
        server: String,
        method: String,
        code: i64,
        digest: String,
    },
    #[error("MCP transport error for {server}: {message}")]
    Transport { server: String, message: String },
    #[error(
        "stale MCP binding for generation {binding_generation}; active generation is {active_generation}"
    )]
    StaleBinding {
        binding_generation: u64,
        active_generation: u64,
    },
    #[error("MCP catalog is unavailable: {0}")]
    Unavailable(String),
}

impl McpError {
    pub fn transport(server: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Transport {
            server: server.into(),
            message: message.into(),
        }
    }
}
