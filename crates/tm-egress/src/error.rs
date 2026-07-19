use tm_host::HostError;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum EgressError {
    #[error("egress is disabled")]
    Disabled,
    #[error("invalid egress configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid egress request: {0}")]
    InvalidRequest(String),
    #[error("egress policy denied: {0}")]
    Denied(String),
    #[error("egress target is revoked: {0}")]
    Revoked(String),
    #[error("egress budget exceeded: {0}")]
    Budget(String),
    #[error("egress request timed out")]
    Timeout,
    #[error("egress DNS resolution failed")]
    Dns,
    #[error("egress transport failed")]
    Transport,
    #[error("egress response was not UTF-8 text")]
    NonUtf8,
    #[error("secret handle is invalid for this session or destination")]
    InvalidSecretHandle,
    #[error("configured secret is unavailable")]
    SecretUnavailable,
    #[error("egress audit persistence failed")]
    Audit,
    #[error("egress durable state failed: {0}")]
    Durability(String),
}

impl EgressError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::InvalidConfig(_) => "invalid_config",
            Self::InvalidRequest(_) => "invalid_request",
            Self::Denied(_) => "denied",
            Self::Revoked(_) => "revoked",
            Self::Budget(_) => "budget_exceeded",
            Self::Timeout => "timeout",
            Self::Dns => "dns_failed",
            Self::Transport => "transport_failed",
            Self::NonUtf8 => "non_utf8_response",
            Self::InvalidSecretHandle => "invalid_secret_handle",
            Self::SecretUnavailable => "secret_unavailable",
            Self::Audit => "audit_failed",
            Self::Durability(_) => "durability_failed",
        }
    }

    pub fn into_host(self) -> HostError {
        match self {
            Self::InvalidConfig(message) | Self::InvalidRequest(message) => {
                HostError::InvalidArgs(message)
            }
            Self::Budget(message) => HostError::QuotaExceeded(message),
            Self::Timeout => HostError::Timeout("http request".into()),
            Self::Transport | Self::Dns | Self::NonUtf8 => HostError::HostCall(self.to_string()),
            Self::Audit => HostError::HostCall("egress audit persistence failed".into()),
            Self::Durability(_) => HostError::HostCall("egress durable state failed".into()),
            Self::Disabled
            | Self::Denied(_)
            | Self::Revoked(_)
            | Self::InvalidSecretHandle
            | Self::SecretUnavailable => HostError::CapabilityDenied(self.to_string()),
        }
    }
}

pub type Result<T, E = EgressError> = std::result::Result<T, E>;
