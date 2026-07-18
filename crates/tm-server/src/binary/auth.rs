use std::net::{IpAddr, SocketAddr};

use ipnet::IpNet;
use tm_server::{AuthConfig, DeviceAuthConfig, ForwardedAuthConfig};

use super::{
    BoxError,
    config::{env_flag, required_env},
};

pub(super) fn server_auth_config(
    addr: SocketAddr,
    owner_subject: &str,
) -> Result<AuthConfig, BoxError> {
    let mode = std::env::var("TM_AUTH_MODE").unwrap_or_else(|_| "device".to_string());
    let public_url = std::env::var("TM_PUBLIC_BASE_URL").ok();
    let allow_insecure = env_flag("TM_ALLOW_INSECURE_HTTP");
    if allow_insecure && !cfg!(debug_assertions) {
        return Err("TM_ALLOW_INSECURE_HTTP is available only in debug builds".into());
    }
    let allowed_origin = public_url.as_deref().map(public_origin).transpose()?;
    validate_bind_security(addr, allow_insecure)?;

    match mode.trim().to_ascii_lowercase().as_str() {
        "device" | "" => Ok(AuthConfig::Device(DeviceAuthConfig {
            cookie_name: tm_server::auth::DEFAULT_DEVICE_COOKIE.to_string(),
            secure_cookie: public_url
                .as_deref()
                .is_some_and(|url| url.trim().starts_with("https://")),
            owner_subject: owner_subject.to_string(),
            bootstrap_token_hash: std::env::var("TM_AUTH_BOOTSTRAP_TOKEN")
                .ok()
                .filter(|token| !token.trim().is_empty())
                .map(|token| tm_server::auth::hash_secret(token.trim())),
            allow_loopback_pairing: addr.ip().is_loopback(),
            allowed_origin,
        })),
        "bearer" => {
            let token = required_env("TM_AUTH_TOKEN")?;
            Ok(AuthConfig::BearerToken(token))
        }
        "forwarded" => {
            let user_header = required_env("TM_AUTH_FORWARDED_USER_HEADER")?;
            let trusted_proxy_networks = std::env::var("TM_AUTH_TRUSTED_PROXY_CIDRS")
                .or_else(|_| std::env::var("TM_AUTH_TRUSTED_PROXY_IPS"))
                .map_err(|_| {
                    "TM_AUTH_TRUSTED_PROXY_CIDRS is required for forwarded auth (TM_AUTH_TRUSTED_PROXY_IPS is a legacy alias)"
                })?;
            let trusted_proxy_cidrs = trusted_proxy_networks
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    if let Ok(network) = value.parse::<IpNet>() {
                        Ok(network)
                    } else {
                        value
                            .parse::<IpAddr>()
                            .map(IpNet::from)
                            .map_err(|error| format!("invalid trusted proxy CIDR {value}: {error}"))
                    }
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if trusted_proxy_cidrs.is_empty() {
                return Err("TM_AUTH_TRUSTED_PROXY_CIDRS must contain at least one network".into());
            }
            let expected_user = std::env::var("TM_AUTH_FORWARDED_EXPECTED_USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| owner_subject.to_string());
            if expected_user != owner_subject {
                return Err("TM_AUTH_FORWARDED_EXPECTED_USER must match TM_OWNER_SUBJECT".into());
            }
            Ok(AuthConfig::Forwarded(ForwardedAuthConfig {
                user_header,
                expected_user: Some(expected_user),
                trusted_proxy_cidrs,
            }))
        }
        "no_auth" | "none" => {
            if !addr.ip().is_loopback() {
                return Err("TM_AUTH_MODE=no_auth is restricted to loopback binds".into());
            }
            Ok(AuthConfig::NoAuth)
        }
        other => Err(format!("unsupported TM_AUTH_MODE {other}").into()),
    }
}

fn validate_bind_security(addr: SocketAddr, allow_insecure: bool) -> Result<(), BoxError> {
    if !addr.ip().is_loopback() && !allow_insecure {
        return Err(
            "tm-server serves plain HTTP and must bind to loopback behind an HTTPS proxy or Tailscale Serve; TM_PUBLIC_BASE_URL does not secure a non-loopback bind (TM_ALLOW_INSECURE_HTTP=1 is debug-only)"
                .into(),
        );
    }
    Ok(())
}

fn public_origin(url: &str) -> Result<String, BoxError> {
    let (scheme, rest) = url
        .trim()
        .split_once("://")
        .ok_or("TM_PUBLIC_BASE_URL must include http:// or https://")?;
    if !matches!(scheme, "http" | "https") {
        return Err("TM_PUBLIC_BASE_URL must use http or https".into());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("TM_PUBLIC_BASE_URL must include a host and no userinfo".into());
    }
    Ok(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_http_bind_is_loopback_only_without_debug_override() {
        assert!(validate_bind_security("127.0.0.1:8787".parse().unwrap(), false).is_ok());
        let error = validate_bind_security("0.0.0.0:8787".parse().unwrap(), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must bind to loopback"));
        assert!(error.contains("TM_PUBLIC_BASE_URL does not secure"));
        assert!(validate_bind_security("0.0.0.0:8787".parse().unwrap(), true).is_ok());
    }
}
