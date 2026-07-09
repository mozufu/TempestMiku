use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub type Result<T, E = HostError> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HostError {
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    #[error("approval denied: {0}")]
    ApprovalDenied(String),
    #[error("approval timed out: {0}")]
    ApprovalTimeout(String),
    #[error("unknown resource scheme: {scheme}; registered: {registered:?}")]
    UnknownScheme {
        scheme: String,
        registered: Vec<String>,
    },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("output truncated: {0}")]
    OutputTruncated(String),
    #[error("host call error: {0}")]
    HostCall(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostErrorPayload {
    pub name: String,
    pub message: String,
    pub capability: Option<String>,
    pub path: Option<String>,
    pub uri: Option<String>,
    pub retryable: bool,
    pub details: Value,
}

impl HostError {
    pub fn sdk_name(&self) -> &'static str {
        match self {
            Self::CapabilityDenied(_) | Self::UnknownScheme { .. } => "CapabilityDeniedError",
            Self::ApprovalDenied(_) => "ApprovalDeniedError",
            Self::ApprovalTimeout(_) => "ApprovalTimeoutError",
            Self::NotFound(_) => "NotFoundError",
            Self::InvalidArgs(_) => "InvalidArgsError",
            Self::InvalidPath(_) => "InvalidPathError",
            Self::NotImplemented(_) => "NotImplementedError",
            Self::QuotaExceeded(_) => "QuotaExceededError",
            Self::Timeout(_) => "TimeoutError",
            Self::OutputTruncated(_) => "OutputTruncatedError",
            Self::HostCall(_) => "HostCallError",
        }
    }

    pub fn to_payload(&self) -> HostErrorPayload {
        let (capability, path, uri, retryable, details) = match self {
            Self::CapabilityDenied(capability) => (
                Some(capability.clone()),
                None,
                None,
                false,
                json!({ "capability": capability }),
            ),
            Self::ApprovalDenied(action) => (None, None, None, false, json!({ "action": action })),
            Self::ApprovalTimeout(action) => (None, None, None, true, json!({ "action": action })),
            Self::UnknownScheme { scheme, registered } => (
                None,
                None,
                Some(format!("{scheme}://")),
                false,
                json!({ "scheme": scheme, "registered": registered }),
            ),
            Self::NotFound(target) => (None, None, None, false, json!({ "target": target })),
            Self::InvalidArgs(message) => (None, None, None, false, json!({ "reason": message })),
            Self::InvalidPath(path) => (
                None,
                Some(path.clone()),
                None,
                false,
                json!({ "path": path }),
            ),
            Self::NotImplemented(feature) => {
                (None, None, None, false, json!({ "feature": feature }))
            }
            Self::QuotaExceeded(quota) => (None, None, None, true, json!({ "quota": quota })),
            Self::Timeout(operation) => (None, None, None, true, json!({ "operation": operation })),
            Self::OutputTruncated(target) => (None, None, None, false, json!({ "target": target })),
            Self::HostCall(message) => (None, None, None, false, json!({ "reason": message })),
        };
        HostErrorPayload {
            name: self.sdk_name().to_string(),
            message: self.to_string(),
            capability,
            path,
            uri,
            retryable,
            details,
        }
    }
}
