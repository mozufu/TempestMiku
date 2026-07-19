use std::collections::BTreeMap;

use tm_host::{EgressConfig, EgressDestinationConfig, EgressSecretConfig, EgressSecretInjection};
use url::Url;

use crate::{EgressError, Result};

pub const MAX_URL_BYTES: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 32;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_REDIRECTS: u8 = 5;
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

const FORBIDDEN_CALLER_HEADERS: &[&str] = &[
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
];

#[derive(Debug, Clone)]
pub(crate) struct ValidatedConfig {
    pub enabled: bool,
    pub session_limits: tm_host::EgressSessionLimits,
    pub destinations: BTreeMap<String, EgressDestinationConfig>,
    pub secrets: BTreeMap<String, EgressSecretConfig>,
}

#[derive(Debug, Clone)]
pub(crate) struct AuthorizedDestination {
    pub policy: EgressDestinationConfig,
    pub url: Url,
}

pub(crate) fn validate_config(config: EgressConfig, allow_http: bool) -> Result<ValidatedConfig> {
    validate_session_limits(&config.session_limits)?;
    let mut destinations = BTreeMap::new();
    for mut destination in config.destinations {
        validate_identifier("destination id", &destination.id)?;
        if destination.version == 0 {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} version must be positive",
                destination.id
            )));
        }
        destination.scheme.make_ascii_lowercase();
        destination.host.make_ascii_lowercase();
        if destination.scheme != "https" && !(allow_http && destination.scheme == "http") {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} must use HTTPS",
                destination.id
            )));
        }
        let origin = Url::parse(&format!(
            "{}://{}:{}/",
            destination.scheme, destination.host, destination.port
        ))
        .map_err(|_| {
            EgressError::InvalidConfig(format!(
                "destination {} has an invalid origin",
                destination.id
            ))
        })?;
        if origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || destination
                .host
                .chars()
                .any(|character| matches!(character, '/' | '@' | '?' | '#'))
        {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} has an invalid host",
                destination.id
            )));
        }
        destination.host = origin
            .host_str()
            .expect("validated origin has host")
            .to_ascii_lowercase();
        if destination.port == 0 {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} port must be positive",
                destination.id
            )));
        }
        if destination.path_prefixes.is_empty() {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} needs at least one path prefix",
                destination.id
            )));
        }
        for prefix in &destination.path_prefixes {
            if !prefix.starts_with('/')
                || prefix
                    .chars()
                    .any(|character| matches!(character, '?' | '#' | '\r' | '\n'))
                || has_ambiguous_path_encoding(prefix)
            {
                return Err(EgressError::InvalidConfig(format!(
                    "destination {} has an invalid path prefix",
                    destination.id
                )));
            }
        }
        if destination.methods.is_empty() {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} needs at least one method",
                destination.id
            )));
        }
        destination.methods = destination
            .methods
            .into_iter()
            .map(|method| method.to_ascii_uppercase())
            .collect();
        if destination
            .methods
            .iter()
            .any(|method| !valid_method(method))
        {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} has an invalid method",
                destination.id
            )));
        }
        if destination.max_redirects > MAX_REDIRECTS {
            return Err(EgressError::InvalidConfig(format!(
                "destination {} exceeds the redirect cap",
                destination.id
            )));
        }
        validate_destination_limits(&destination)?;
        destination.allowed_request_headers = destination
            .allowed_request_headers
            .into_iter()
            .map(|header| header.to_ascii_lowercase())
            .collect();
        for header in &destination.allowed_request_headers {
            validate_header_name(header)?;
            if caller_header_forbidden(header) {
                return Err(EgressError::InvalidConfig(format!(
                    "destination {} cannot allow caller header {header}",
                    destination.id
                )));
            }
        }
        if destinations
            .insert(destination.id.clone(), destination)
            .is_some()
        {
            return Err(EgressError::InvalidConfig(
                "duplicate destination id".into(),
            ));
        }
    }

    for destination in destinations.values() {
        for redirect in &destination.redirect_to {
            if !destinations.contains_key(redirect) {
                return Err(EgressError::InvalidConfig(format!(
                    "destination {} redirects to unknown destination {redirect}",
                    destination.id
                )));
            }
        }
    }

    let mut secrets = BTreeMap::new();
    for mut secret in config.secrets {
        validate_identifier("secret id", &secret.id)?;
        if secret.version == 0 {
            return Err(EgressError::InvalidConfig(format!(
                "secret {} version must be positive",
                secret.id
            )));
        }
        validate_env_name(&secret.env)?;
        if secret.destinations.is_empty()
            || secret
                .destinations
                .iter()
                .any(|id| !destinations.contains_key(id))
        {
            return Err(EgressError::InvalidConfig(format!(
                "secret {} must reference only configured destinations",
                secret.id
            )));
        }
        match &mut secret.injection {
            EgressSecretInjection::AuthorizationBearer => {}
            EgressSecretInjection::Header { name, prefix } => {
                name.make_ascii_lowercase();
                validate_header_name(name)?;
                if caller_header_forbidden(name) {
                    return Err(EgressError::InvalidConfig(format!(
                        "secret {} has a forbidden injection header",
                        secret.id
                    )));
                }
                if prefix.len() > 256
                    || prefix
                        .chars()
                        .any(|character| matches!(character, '\r' | '\n'))
                {
                    return Err(EgressError::InvalidConfig(format!(
                        "secret {} has an invalid header prefix",
                        secret.id
                    )));
                }
            }
        }
        if secrets.insert(secret.id.clone(), secret).is_some() {
            return Err(EgressError::InvalidConfig("duplicate secret id".into()));
        }
    }

    if config.enabled && destinations.is_empty() {
        return Err(EgressError::InvalidConfig(
            "enabled egress requires a destination".into(),
        ));
    }
    Ok(ValidatedConfig {
        enabled: config.enabled,
        session_limits: config.session_limits,
        destinations,
        secrets,
    })
}

impl ValidatedConfig {
    pub(crate) fn authorize(&self, url: &str, method: &str) -> Result<AuthorizedDestination> {
        if !self.enabled {
            return Err(EgressError::Disabled);
        }
        if url.len() > MAX_URL_BYTES {
            return Err(EgressError::InvalidRequest("URL exceeds 8 KiB".into()));
        }
        let url = Url::parse(url)
            .map_err(|_| EgressError::InvalidRequest("URL must be absolute".into()))?;
        if url.cannot_be_a_base()
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || has_ambiguous_path_encoding(url.path())
        {
            return Err(EgressError::Denied("URL shape is not allowed".into()));
        }
        let method = method.to_ascii_uppercase();
        let host = url
            .host_str()
            .expect("validated URL has host")
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| EgressError::Denied("URL has no effective port".into()))?;
        let policy = self
            .destinations
            .values()
            .find(|destination| {
                destination.scheme == url.scheme()
                    && destination.host == host
                    && destination.port == port
                    && destination.methods.contains(&method)
                    && destination
                        .path_prefixes
                        .iter()
                        .any(|prefix| path_prefix_matches(prefix, url.path()))
            })
            .cloned()
            .ok_or_else(|| EgressError::Denied("destination is not allowlisted".into()))?;
        Ok(AuthorizedDestination { policy, url })
    }
}

pub(crate) fn validate_request_headers(
    policy: &EgressDestinationConfig,
    headers: &BTreeMap<String, String>,
) -> Result<()> {
    if headers.len() > MAX_HEADERS {
        return Err(EgressError::InvalidRequest(
            "too many request headers".into(),
        ));
    }
    let mut bytes = 0usize;
    for (name, value) in headers {
        let normalized = name.to_ascii_lowercase();
        validate_header_name(&normalized)
            .map_err(|_| EgressError::InvalidRequest("invalid request header".into()))?;
        if caller_header_forbidden(&normalized) {
            return Err(EgressError::Denied(
                "caller-supplied credential or hop-by-hop header is forbidden".into(),
            ));
        }
        if !policy.allowed_request_headers.contains(&normalized) {
            return Err(EgressError::Denied(
                "caller-supplied header is not allowlisted".into(),
            ));
        }
        if value
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
        {
            return Err(EgressError::InvalidRequest(
                "invalid request header value".into(),
            ));
        }
        bytes = bytes
            .checked_add(name.len())
            .and_then(|total| total.checked_add(value.len()))
            .and_then(|total| total.checked_add(4))
            .ok_or_else(|| EgressError::InvalidRequest("request headers are too large".into()))?;
    }
    if bytes > MAX_HEADER_BYTES {
        return Err(EgressError::InvalidRequest(
            "request headers exceed 16 KiB".into(),
        ));
    }
    Ok(())
}

pub(crate) fn caller_header_forbidden(name: &str) -> bool {
    FORBIDDEN_CALLER_HEADERS
        .iter()
        .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
}

fn validate_session_limits(limits: &tm_host::EgressSessionLimits) -> Result<()> {
    if limits.max_requests == 0
        || limits.max_request_bytes == 0
        || limits.max_response_bytes == 0
        || limits.max_time_ms == 0
    {
        return Err(EgressError::InvalidConfig(
            "session egress limits must be positive".into(),
        ));
    }
    Ok(())
}

fn validate_destination_limits(destination: &EgressDestinationConfig) -> Result<()> {
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
        return Err(EgressError::InvalidConfig(format!(
            "destination {} has invalid limits",
            destination.id
        )));
    }
    Ok(())
}

fn validate_identifier(kind: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(EgressError::InvalidConfig(format!(
            "{kind} must be 1-128 safe ASCII characters"
        )));
    }
    Ok(())
}

fn validate_env_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(EgressError::InvalidConfig(
            "secret env name must be 1-128 uppercase ASCII characters".into(),
        ));
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
        return Err(EgressError::InvalidConfig(
            "invalid HTTP header name".into(),
        ));
    }
    Ok(())
}

fn valid_method(method: &str) -> bool {
    !method.is_empty()
        && method.len() <= 16
        && method
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
}

fn path_prefix_matches(prefix: &str, path: &str) -> bool {
    if prefix == "/" || prefix == path {
        return true;
    }
    let Some(remaining) = path.strip_prefix(prefix) else {
        return false;
    };
    prefix.ends_with('/') || remaining.starts_with('/')
}

fn has_ambiguous_path_encoding(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // `%25` is deliberately rejected even when the next layer would decode to a benign byte.
    // Otherwise a downstream proxy/framework performing a second (or later) decode can turn
    // `%252e`, `%252f`, `%255c`, or split forms such as `%25%32%65` into traversal delimiters.
    // Raw dot segments and backslashes are likewise not valid policy syntax.
    path.contains('\\')
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
        || ["%00", "%25", "%2e", "%2f", "%5c"]
            .iter()
            .any(|needle| lower.contains(needle))
}
