use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{HostError, Result};

/// Declarative host policy for the production egress boundary.
///
/// This module intentionally contains data contracts only. DNS resolution, HTTP transport,
/// secret lookup, budgets, and runtime revocation live in the concrete `tm-egress` crate.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EgressConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub session_limits: EgressSessionLimits,
    #[serde(default)]
    pub destinations: Vec<EgressDestinationConfig>,
    #[serde(default)]
    pub secrets: Vec<EgressSecretConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EgressSessionLimits {
    #[serde(default = "default_session_requests")]
    pub max_requests: u32,
    #[serde(default = "default_session_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_session_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_session_time_ms")]
    pub max_time_ms: u64,
}

impl Default for EgressSessionLimits {
    fn default() -> Self {
        Self {
            max_requests: default_session_requests(),
            max_request_bytes: default_session_request_bytes(),
            max_response_bytes: default_session_response_bytes(),
            max_time_ms: default_session_time_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EgressDestinationConfig {
    pub id: String,
    #[serde(default = "default_policy_version")]
    pub version: u64,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    #[serde(default = "default_path_prefixes")]
    pub path_prefixes: Vec<String>,
    #[serde(default = "default_methods")]
    pub methods: BTreeSet<String>,
    #[serde(default)]
    pub redirect_to: BTreeSet<String>,
    #[serde(default)]
    pub max_redirects: u8,
    #[serde(default)]
    pub allow_private_ips: bool,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_destination_requests")]
    pub max_requests_per_session: u32,
    #[serde(default = "default_destination_request_bytes")]
    pub max_request_bytes_per_session: usize,
    #[serde(default = "default_destination_response_bytes")]
    pub max_response_bytes_per_session: usize,
    #[serde(default = "default_destination_time_ms")]
    pub max_time_ms_per_session: u64,
    /// Optional caller-supplied headers. Secret-bearing and hop-by-hop headers are always denied.
    #[serde(default)]
    pub allowed_request_headers: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EgressSecretConfig {
    pub id: String,
    #[serde(default = "default_policy_version")]
    pub version: u64,
    /// Name of the host environment variable. The value is never part of this config contract.
    pub env: String,
    pub destinations: BTreeSet<String>,
    pub injection: EgressSecretInjection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EgressSecretInjection {
    AuthorizationBearer,
    Header {
        name: String,
        #[serde(default)]
        prefix: String,
    },
}

/// The public value returned by `secrets.use`. The token is a short-lived reference into the
/// host broker; it is neither the secret value nor sufficient authority without the current
/// session grant and destination policy.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretHandle {
    pub token: String,
}

impl std::fmt::Debug for SecretHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretHandle")
            .field("token", &"[opaque]")
            .finish()
    }
}

impl EgressConfig {
    pub fn destination_map(&self) -> BTreeMap<&str, &EgressDestinationConfig> {
        self.destinations
            .iter()
            .map(|destination| (destination.id.as_str(), destination))
            .collect()
    }

    /// Validate the serialized production policy before application startup constructs the
    /// concrete transport. Runtime DNS, revocation, and budget checks remain in `tm-egress`.
    pub fn validate(&self) -> Result<()> {
        if self.session_limits.max_requests == 0
            || self.session_limits.max_request_bytes == 0
            || self.session_limits.max_response_bytes == 0
            || self.session_limits.max_time_ms == 0
        {
            return Err(invalid("egress session limits must be positive"));
        }
        if self.enabled && self.destinations.is_empty() {
            return Err(invalid("enabled egress requires at least one destination"));
        }

        let mut destination_ids = BTreeSet::new();
        for destination in &self.destinations {
            validate_identifier("egress destination id", &destination.id)?;
            if !destination_ids.insert(destination.id.as_str()) {
                return Err(invalid("egress destination ids must be unique"));
            }
            if destination.version == 0 {
                return Err(invalid("egress destination versions must be positive"));
            }
            if destination.scheme != "https" {
                return Err(invalid("production egress destinations must use https"));
            }
            if destination.port == 0 {
                return Err(invalid("egress destination ports must be positive"));
            }
            let origin = Url::parse(&format!(
                "{}://{}:{}/",
                destination.scheme, destination.host, destination.port
            ))
            .map_err(|_| invalid("egress destination origin is invalid"))?;
            if origin.host_str().is_none()
                || !origin.username().is_empty()
                || origin.password().is_some()
                || destination
                    .host
                    .chars()
                    .any(|character| matches!(character, '/' | '@' | '?' | '#'))
            {
                return Err(invalid("egress destination host is invalid"));
            }
            if destination.path_prefixes.is_empty()
                || destination.path_prefixes.iter().any(|prefix| {
                    !prefix.starts_with('/')
                        || prefix
                            .chars()
                            .any(|character| matches!(character, '?' | '#' | '\r' | '\n'))
                        || has_ambiguous_path_encoding(prefix)
                })
            {
                return Err(invalid(
                    "egress destinations require unambiguous absolute path prefixes",
                ));
            }
            if destination.methods.is_empty()
                || destination.methods.iter().any(|method| {
                    method.is_empty()
                        || method.len() > 16
                        || !method
                            .bytes()
                            .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
                })
            {
                return Err(invalid("egress destination methods are invalid"));
            }
            if destination.max_redirects > 5 {
                return Err(invalid("egress redirect cap must not exceed 5"));
            }
            if destination.request_timeout_ms == 0
                || destination.request_timeout_ms > 120_000
                || destination.connect_timeout_ms == 0
                || destination.connect_timeout_ms > destination.request_timeout_ms
                || destination.max_request_bytes == 0
                || destination.max_response_bytes == 0
                || destination.max_requests_per_session == 0
                || destination.max_request_bytes_per_session == 0
                || destination.max_response_bytes_per_session == 0
                || destination.max_time_ms_per_session == 0
            {
                return Err(invalid("egress destination limits are invalid"));
            }
            for header in &destination.allowed_request_headers {
                validate_header_name(header)?;
                if caller_header_forbidden(header) {
                    return Err(invalid(
                        "egress caller headers cannot include credentials or hop-by-hop headers",
                    ));
                }
            }
        }
        for destination in &self.destinations {
            if destination
                .redirect_to
                .iter()
                .any(|id| !destination_ids.contains(id.as_str()))
            {
                return Err(invalid(
                    "egress redirect targets must name configured destinations",
                ));
            }
        }

        let mut secret_ids = BTreeSet::new();
        for secret in &self.secrets {
            validate_identifier("egress secret id", &secret.id)?;
            if !secret_ids.insert(secret.id.as_str()) {
                return Err(invalid("egress secret ids must be unique"));
            }
            if secret.version == 0 {
                return Err(invalid("egress secret versions must be positive"));
            }
            if secret.env.is_empty()
                || secret.env.len() > 128
                || !secret
                    .env
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(invalid(
                    "egress secret env names must be 1-128 uppercase ASCII characters",
                ));
            }
            if secret.destinations.is_empty()
                || secret
                    .destinations
                    .iter()
                    .any(|id| !destination_ids.contains(id.as_str()))
            {
                return Err(invalid("egress secrets must name configured destinations"));
            }
            if let EgressSecretInjection::Header { name, prefix } = &secret.injection {
                validate_header_name(name)?;
                if caller_header_forbidden(name)
                    || prefix.len() > 256
                    || prefix
                        .chars()
                        .any(|character| matches!(character, '\r' | '\n'))
                {
                    return Err(invalid("egress secret header injection is invalid"));
                }
            }
        }
        Ok(())
    }

    /// Exact grants installed into an already network-enabled mode. Disabled policy returns no
    /// grants; destination and secret identifiers never become namespace wildcards.
    pub fn turn_capabilities(&self) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut capabilities = vec!["http.request".to_string()];
        capabilities.extend(
            self.destinations
                .iter()
                .map(|destination| format!("egress.destination:{}", destination.id)),
        );
        if !self.secrets.is_empty() {
            capabilities.push("secrets.use".to_string());
            capabilities.extend(
                self.secrets
                    .iter()
                    .map(|secret| format!("secrets.use:{}", secret.id)),
            );
        }
        capabilities.sort();
        capabilities.dedup();
        capabilities
    }
}

fn invalid(message: impl Into<String>) -> HostError {
    HostError::InvalidArgs(message.into())
}

fn validate_identifier(kind: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid(format!(
            "{kind} must be 1-128 safe ASCII characters"
        )));
    }
    Ok(())
}

fn validate_header_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(invalid("egress HTTP header name is invalid"));
    }
    Ok(())
}

fn caller_header_forbidden(name: &str) -> bool {
    [
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
        "host",
        "connection",
        "content-length",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
    ]
    .iter()
    .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
}

fn has_ambiguous_path_encoding(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    path.contains('\\')
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
        || ["%00", "%25", "%2e", "%2f", "%5c"]
            .iter()
            .any(|needle| lower.contains(needle))
}

fn default_policy_version() -> u64 {
    1
}
fn default_session_requests() -> u32 {
    16
}
fn default_session_request_bytes() -> usize {
    1 << 20
}
fn default_session_response_bytes() -> usize {
    8 << 20
}
fn default_session_time_ms() -> u64 {
    120_000
}
fn default_destination_requests() -> u32 {
    8
}
fn default_destination_request_bytes() -> usize {
    512 << 10
}
fn default_destination_response_bytes() -> usize {
    4 << 20
}
fn default_destination_time_ms() -> u64 {
    60_000
}
fn default_request_timeout_ms() -> u64 {
    15_000
}
fn default_connect_timeout_ms() -> u64 {
    5_000
}
fn default_request_bytes() -> usize {
    256 << 10
}
fn default_response_bytes() -> usize {
    1 << 20
}
fn default_path_prefixes() -> Vec<String> {
    vec!["/".to_string()]
}
fn default_methods() -> BTreeSet<String> {
    BTreeSet::from(["GET".to_string()])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn host_config_egress_is_disabled_by_default() {
        let config: crate::P0HostConfig = serde_json::from_value(json!({})).unwrap();
        assert!(!config.egress.enabled);
        assert!(config.egress.destinations.is_empty());
        config.validate().unwrap();
    }

    #[test]
    fn host_config_rejects_unsafe_or_dangling_egress_policy() {
        for value in [
            json!({
                "egress": {
                    "enabled": true,
                    "destinations": []
                }
            }),
            json!({
                "egress": {
                    "enabled": true,
                    "destinations": [{
                        "id": "live",
                        "scheme": "http",
                        "host": "example.com",
                        "port": 80
                    }]
                }
            }),
            json!({
                "egress": {
                    "destinations": [{
                        "id": "live",
                        "scheme": "https",
                        "host": "example.com",
                        "port": 443
                    }],
                    "secrets": [{
                        "id": "api",
                        "env": "API_TOKEN",
                        "destinations": ["missing"],
                        "injection": {"kind": "authorization_bearer"}
                    }]
                }
            }),
            json!({
                "egress": {
                    "destinations": [{
                        "id": "live",
                        "scheme": "https",
                        "host": "example.com",
                        "port": 443,
                        "path_prefixes": ["/allowed/%252e%252e/secret"]
                    }]
                }
            }),
        ] {
            let config: crate::P0HostConfig = serde_json::from_value(value).unwrap();
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn host_config_accepts_exact_https_destination_and_secret_reference() {
        let config: crate::P0HostConfig = serde_json::from_value(json!({
            "egress": {
                "enabled": true,
                "destinations": [{
                    "id": "research",
                    "scheme": "https",
                    "host": "api.example.com",
                    "port": 443,
                    "path_prefixes": ["/v1/search"],
                    "methods": ["GET", "POST"]
                }],
                "secrets": [{
                    "id": "research_api",
                    "env": "RESEARCH_API_TOKEN",
                    "destinations": ["research"],
                    "injection": {"kind": "authorization_bearer"}
                }]
            }
        }))
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn host_config_never_accepts_inline_secret_literals() {
        let config = serde_json::from_value::<crate::P0HostConfig>(json!({
            "egress": {
                "enabled": true,
                "destinations": [{
                    "id": "research",
                    "scheme": "https",
                    "host": "api.example.com",
                    "port": 443
                }],
                "secrets": [{
                    "id": "research_api",
                    "env": "RESEARCH_API_TOKEN",
                    "value": "must-never-enter-policy-memory",
                    "destinations": ["research"],
                    "injection": {"kind": "authorization_bearer"}
                }]
            }
        }));
        assert!(config.is_err());
    }
}
