use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{MCP_PROTOCOL_VERSION, error::McpError};

/// Trusted, operator-authored MCP startup policy. It contains only references to P9 destination
/// and secret policy entries; credential values never cross this contract.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct McpRuntimeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub servers: Vec<McpConfiguredServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct McpConfiguredServer {
    /// Stable local alias; remote names and descriptions never define local authority.
    pub alias: String,
    pub url: String,
    pub destination_id: String,
    #[serde(default)]
    pub secret_id: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub allow: McpObjectAllowlist,
}

impl McpRuntimeConfig {
    pub fn validate(&self, bounds: &McpBounds) -> crate::Result<()> {
        bounds.validate()?;
        if self.enabled && self.servers.is_empty() {
            return Err(McpError::InvalidConfig(
                "enabled MCP runtime requires at least one server".to_string(),
            ));
        }
        if self.servers.len() > bounds.max_servers {
            return Err(McpError::Bounds {
                target: "MCP configured servers".to_string(),
                limit: format!(
                    "{} servers exceeds {}",
                    self.servers.len(),
                    bounds.max_servers
                ),
            });
        }
        let mut aliases = BTreeSet::new();
        for server in &self.servers {
            crate::validate::validate_server_alias(&server.alias)?;
            if !aliases.insert(server.alias.as_str()) {
                return Err(McpError::Collision(format!(
                    "duplicate server alias {}",
                    server.alias
                )));
            }
            crate::http_transport::validate_server(&server.http_config())?;
        }
        Ok(())
    }

    /// Validate every MCP transport reference against the already validated P9 host policy.
    /// This catches alias-to-destination drift before startup performs any network discovery.
    pub fn validate_egress(&self, egress: &tm_host::EgressConfig) -> crate::Result<()> {
        self.validate(&McpBounds::default())?;
        if !self.enabled {
            return Ok(());
        }
        if !egress.enabled {
            return Err(McpError::InvalidConfig(
                "enabled MCP runtime requires enabled P9 egress".to_string(),
            ));
        }
        let destinations = egress.destination_map();
        let secrets = egress
            .secrets
            .iter()
            .map(|secret| (secret.id.as_str(), secret))
            .collect::<BTreeMap<_, _>>();
        let required_headers = [
            "accept",
            "content-type",
            "last-event-id",
            "mcp-protocol-version",
            "mcp-session-id",
        ];
        for server in &self.servers {
            let destination = destinations
                .get(server.destination_id.as_str())
                .copied()
                .ok_or_else(|| {
                    McpError::InvalidConfig(format!(
                        "MCP server {} references unknown P9 destination {}",
                        server.alias, server.destination_id
                    ))
                })?;
            let url = url::Url::parse(&server.url).map_err(|_| {
                McpError::InvalidConfig(format!("MCP server {} has an invalid URL", server.alias))
            })?;
            let port = url.port_or_known_default().ok_or_else(|| {
                McpError::InvalidConfig(format!(
                    "MCP server {} URL has no effective port",
                    server.alias
                ))
            })?;
            if url.query().is_some() {
                return Err(McpError::InvalidConfig(format!(
                    "MCP server {} URL must not contain a query; credentials and authority belong in opaque headers",
                    server.alias
                )));
            }
            let methods_allowed = ["POST", "GET"].iter().all(|required| {
                destination
                    .methods
                    .iter()
                    .any(|method| method.eq_ignore_ascii_case(required))
            });
            let path_allowed = destination
                .path_prefixes
                .iter()
                .any(|prefix| path_prefix_matches(prefix, url.path()));
            if url.scheme() != destination.scheme
                || url.host_str() != Some(destination.host.as_str())
                || port != destination.port
                || !methods_allowed
                || !path_allowed
            {
                return Err(McpError::InvalidConfig(format!(
                    "MCP server {} URL is outside exact P9 destination {}",
                    server.alias, server.destination_id
                )));
            }
            if required_headers.iter().any(|required| {
                !destination
                    .allowed_request_headers
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(required))
            }) {
                return Err(McpError::InvalidConfig(format!(
                    "MCP destination {} must allow the bounded Streamable HTTP headers",
                    server.destination_id
                )));
            }
            if let Some(secret_id) = &server.secret_id {
                let secret = secrets.get(secret_id.as_str()).copied().ok_or_else(|| {
                    McpError::InvalidConfig(format!(
                        "MCP server {} references unknown P9 secret {}",
                        server.alias, secret_id
                    ))
                })?;
                if !secret.destinations.contains(&server.destination_id) {
                    return Err(McpError::InvalidConfig(format!(
                        "MCP secret {} is not authorized for destination {}",
                        secret_id, server.destination_id
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn specs(&self) -> Vec<McpServerSpec> {
        self.servers
            .iter()
            .map(|server| McpServerSpec {
                alias: server.alias.clone(),
                allow: server.allow.clone(),
            })
            .collect()
    }

    pub fn http_servers(&self) -> Vec<crate::McpHttpServerConfig> {
        self.servers
            .iter()
            .map(McpConfiguredServer::http_config)
            .collect()
    }
}

fn path_prefix_matches(prefix: &str, path: &str) -> bool {
    if prefix == "/" || path == prefix {
        return true;
    }
    path.strip_prefix(prefix)
        .is_some_and(|suffix| prefix.ends_with('/') || suffix.starts_with('/'))
}

impl McpConfiguredServer {
    fn http_config(&self) -> crate::McpHttpServerConfig {
        crate::McpHttpServerConfig {
            alias: self.alias.clone(),
            url: self.url.clone(),
            destination_id: self.destination_id.clone(),
            secret_id: self.secret_id.clone(),
            timeout_ms: self.timeout_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpBounds {
    pub max_servers: usize,
    pub max_items_per_server: usize,
    pub max_total_items: usize,
    pub max_pages_per_list: usize,
    pub max_schema_bytes: usize,
    pub max_schema_depth: usize,
    pub max_schema_nodes: usize,
    pub max_request_bytes: usize,
    pub max_result_bytes: usize,
    pub max_cursor_bytes: usize,
    pub max_name_bytes: usize,
    pub max_uri_bytes: usize,
    pub max_content_items: usize,
}

impl Default for McpBounds {
    fn default() -> Self {
        Self {
            max_servers: 8,
            max_items_per_server: 512,
            max_total_items: 2_048,
            max_pages_per_list: 32,
            max_schema_bytes: 64 * 1024,
            max_schema_depth: 32,
            max_schema_nodes: 4_096,
            max_request_bytes: 256 * 1024,
            max_result_bytes: 512 * 1024,
            max_cursor_bytes: 512,
            max_name_bytes: 128,
            max_uri_bytes: 4 * 1024,
            max_content_items: 128,
        }
    }
}

impl McpBounds {
    pub(crate) fn validate(&self) -> crate::Result<()> {
        let fields = [
            ("max_servers", self.max_servers),
            ("max_items_per_server", self.max_items_per_server),
            ("max_total_items", self.max_total_items),
            ("max_pages_per_list", self.max_pages_per_list),
            ("max_schema_bytes", self.max_schema_bytes),
            ("max_schema_depth", self.max_schema_depth),
            ("max_schema_nodes", self.max_schema_nodes),
            ("max_request_bytes", self.max_request_bytes),
            ("max_result_bytes", self.max_result_bytes),
            ("max_cursor_bytes", self.max_cursor_bytes),
            ("max_name_bytes", self.max_name_bytes),
            ("max_uri_bytes", self.max_uri_bytes),
            ("max_content_items", self.max_content_items),
        ];
        if let Some((name, _)) = fields.into_iter().find(|(_, value)| *value == 0) {
            return Err(McpError::InvalidConfig(format!(
                "{name} must be greater than zero"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolPolicy {
    /// Mutability is trusted local policy. Remote MCP annotations never weaken this decision.
    #[serde(default)]
    pub mutation: bool,
}

impl McpToolPolicy {
    pub const fn read_only() -> Self {
        Self { mutation: false }
    }

    pub const fn mutation() -> Self {
        Self { mutation: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpObjectAllowlist {
    #[serde(default)]
    pub tools: BTreeMap<String, McpToolPolicy>,
    /// Exact, server-native resource URIs.
    #[serde(default)]
    pub resources: BTreeSet<String>,
    #[serde(default)]
    pub prompts: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSpec {
    /// Stable local alias used in capability and `mcp://` names. It is not supplied by the server.
    pub alias: String,
    pub allow: McpObjectAllowlist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogView {
    pub generation: u64,
    pub digest: String,
    pub protocol_version: String,
    pub servers: Vec<McpServerView>,
}

impl McpCatalogView {
    pub(crate) fn empty() -> Self {
        Self {
            generation: 0,
            digest: crate::validate::sha256_hex(b"empty-mcp-catalog"),
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            servers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    pub alias: String,
    pub implementation_digest: String,
    pub capabilities: BTreeSet<String>,
    pub tools: Vec<McpToolView>,
    pub resources: Vec<McpResourceView>,
    pub prompts: Vec<McpPromptView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolView {
    pub name: String,
    pub capability: String,
    pub mutation: bool,
    pub input_schema_digest: String,
    pub output_schema_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceView {
    pub local_uri: String,
    pub capability: String,
    pub source_uri_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptView {
    pub name: String,
    pub capability: String,
    pub arguments: Vec<McpPromptArgumentView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptArgumentView {
    pub name: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProvenance {
    pub protocol_version: String,
    pub catalog_generation: u64,
    pub catalog_digest: String,
    pub server: String,
    pub object_kind: String,
    pub object_name: String,
    pub target_digest: String,
    pub payload_digest: String,
    pub payload_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UntrustedMcpData {
    pub kind: String,
    pub trust: String,
    pub provenance: McpProvenance,
    pub data: Value,
}

impl UntrustedMcpData {
    pub(crate) fn new(provenance: McpProvenance, data: Value) -> Self {
        Self {
            kind: "mcp_untrusted_data".to_string(),
            trust: "untrusted".to_string(),
            provenance,
            data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    pub previous_generation: u64,
    pub generation: u64,
    pub digest: String,
    pub servers: usize,
    pub tools: usize,
    pub resources: usize,
    pub prompts: usize,
}
