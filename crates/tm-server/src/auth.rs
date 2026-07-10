use std::{
    collections::BTreeMap,
    fmt,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use async_trait::async_trait;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Result, ServerError};

pub const DEFAULT_DEVICE_COOKIE: &str = "tm_device";
pub const BOOTSTRAP_HEADER: &str = "x-tempestmiku-bootstrap";

#[derive(Clone, Default)]
pub enum AuthConfig {
    #[default]
    NoAuth,
    BearerToken(String),
    Forwarded(ForwardedAuthConfig),
    Device(DeviceAuthConfig),
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAuth => formatter.write_str("NoAuth"),
            Self::BearerToken(_) => formatter
                .debug_tuple("BearerToken")
                .field(&"[REDACTED]")
                .finish(),
            Self::Forwarded(config) => formatter.debug_tuple("Forwarded").field(config).finish(),
            Self::Device(config) => formatter.debug_tuple("Device").field(config).finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ForwardedAuthConfig {
    pub user_header: String,
    pub expected_user: Option<String>,
    pub trusted_proxy_cidrs: Vec<IpNet>,
}

impl ForwardedAuthConfig {
    pub fn trusts(&self, peer_ip: IpAddr) -> bool {
        self.trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(&peer_ip))
    }
}

#[derive(Clone)]
pub struct DeviceAuthConfig {
    pub cookie_name: String,
    pub secure_cookie: bool,
    pub owner_subject: String,
    pub bootstrap_token_hash: Option<String>,
    pub allow_loopback_pairing: bool,
    pub allowed_origin: Option<String>,
}

impl fmt::Debug for DeviceAuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceAuthConfig")
            .field("cookie_name", &self.cookie_name)
            .field("secure_cookie", &self.secure_cookie)
            .field("owner_subject", &self.owner_subject)
            .field(
                "bootstrap_token_hash",
                &self.bootstrap_token_hash.as_ref().map(|_| "[REDACTED]"),
            )
            .field("allow_loopback_pairing", &self.allow_loopback_pairing)
            .field("allowed_origin", &self.allowed_origin)
            .finish()
    }
}

impl Default for DeviceAuthConfig {
    fn default() -> Self {
        Self {
            cookie_name: DEFAULT_DEVICE_COOKIE.to_string(),
            secure_cookie: true,
            owner_subject: "owner".to_string(),
            bootstrap_token_hash: None,
            allow_loopback_pairing: false,
            allowed_origin: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthDeviceRecord {
    pub id: Uuid,
    pub owner_subject: String,
    pub name: String,
    pub platform: String,
    #[serde(skip)]
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for AuthDeviceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthDeviceRecord")
            .field("id", &self.id)
            .field("owner_subject", &self.owner_subject)
            .field("name", &self.name)
            .field("platform", &self.platform)
            .field("token_hash", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .field("last_seen_at", &self.last_seen_at)
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
}

#[derive(Clone)]
pub struct NewAuthDevice {
    pub id: Uuid,
    pub owner_subject: String,
    pub name: String,
    pub platform: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for NewAuthDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewAuthDevice")
            .field("id", &self.id)
            .field("owner_subject", &self.owner_subject)
            .field("name", &self.name)
            .field("platform", &self.platform)
            .field("token_hash", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PairingCodeRecord {
    pub id: Uuid,
    pub code_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_by_device_id: Option<Uuid>,
}

impl fmt::Debug for PairingCodeRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingCodeRecord")
            .field("id", &self.id)
            .field("code_hash", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("consumed_at", &self.consumed_at)
            .field("created_by_device_id", &self.created_by_device_id)
            .finish()
    }
}

#[derive(Clone)]
pub struct NewPairingCode {
    pub id: Uuid,
    pub code_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub created_by_device_id: Option<Uuid>,
}

impl fmt::Debug for NewPairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewPairingCode")
            .field("id", &self.id)
            .field("code_hash", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("created_by_device_id", &self.created_by_device_id)
            .finish()
    }
}

#[async_trait]
pub trait AuthDeviceStore: Send + Sync + 'static {
    async fn auth_device_count(&self) -> Result<usize>;
    async fn authenticate_device(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AuthDeviceRecord>>;
    async fn create_pairing_code(&self, code: NewPairingCode) -> Result<PairingCodeRecord>;
    async fn consume_pairing_code(
        &self,
        code_hash: &str,
        device: NewAuthDevice,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord>;
    async fn auth_devices(&self) -> Result<Vec<AuthDeviceRecord>>;
    async fn revoke_auth_device(
        &self,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryAuthDeviceStore {
    inner: Arc<Mutex<InMemoryAuthInner>>,
}

#[derive(Debug, Default)]
struct InMemoryAuthInner {
    devices: BTreeMap<Uuid, AuthDeviceRecord>,
    pairing_codes: BTreeMap<Uuid, PairingCodeRecord>,
}

#[async_trait]
impl AuthDeviceStore for InMemoryAuthDeviceStore {
    async fn auth_device_count(&self) -> Result<usize> {
        Ok(self.inner.lock().devices.len())
    }

    async fn authenticate_device(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AuthDeviceRecord>> {
        let mut inner = self.inner.lock();
        let Some(device) = inner
            .devices
            .values_mut()
            .find(|device| device.revoked_at.is_none() && device.token_hash == token_hash)
        else {
            return Ok(None);
        };
        device.last_seen_at = now;
        Ok(Some(device.clone()))
    }

    async fn create_pairing_code(&self, code: NewPairingCode) -> Result<PairingCodeRecord> {
        let record = PairingCodeRecord {
            id: code.id,
            code_hash: code.code_hash,
            created_at: code.created_at,
            expires_at: code.expires_at,
            consumed_at: None,
            created_by_device_id: code.created_by_device_id,
        };
        self.inner
            .lock()
            .pairing_codes
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn consume_pairing_code(
        &self,
        code_hash: &str,
        device: NewAuthDevice,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord> {
        let mut inner = self.inner.lock();
        let Some(code) = inner.pairing_codes.values_mut().find(|code| {
            code.code_hash == code_hash && code.consumed_at.is_none() && code.expires_at > now
        }) else {
            return Err(ServerError::Forbidden);
        };
        code.consumed_at = Some(now);
        let record = AuthDeviceRecord {
            id: device.id,
            owner_subject: device.owner_subject,
            name: device.name,
            platform: device.platform,
            token_hash: device.token_hash,
            created_at: device.created_at,
            last_seen_at: device.created_at,
            revoked_at: None,
        };
        inner.devices.insert(record.id, record.clone());
        Ok(record)
    }

    async fn auth_devices(&self) -> Result<Vec<AuthDeviceRecord>> {
        let mut devices = self
            .inner
            .lock()
            .devices
            .values()
            .cloned()
            .collect::<Vec<_>>();
        devices.sort_by_key(|device| std::cmp::Reverse(device.created_at));
        Ok(devices)
    }

    async fn revoke_auth_device(
        &self,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord> {
        let mut inner = self.inner.lock();
        let device = inner
            .devices
            .get_mut(&device_id)
            .ok_or_else(|| ServerError::NotFound(format!("auth device {device_id}")))?;
        device.revoked_at = Some(now);
        Ok(device.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPrincipal {
    Local,
    StaticBearer,
    Forwarded(String),
    Device {
        device: AuthDeviceRecord,
        credential: DeviceCredential,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCredential {
    Bearer,
    Cookie,
}

#[derive(Clone)]
pub struct AuthContext {
    config: AuthConfig,
    devices: Arc<dyn AuthDeviceStore>,
}

impl AuthContext {
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config,
            devices: Arc::new(InMemoryAuthDeviceStore::default()),
        }
    }

    pub fn with_store(mut self, devices: Arc<dyn AuthDeviceStore>) -> Self {
        self.devices = devices;
        self
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    pub fn devices(&self) -> &Arc<dyn AuthDeviceStore> {
        &self.devices
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<AuthPrincipal> {
        match &self.config {
            AuthConfig::NoAuth => Ok(AuthPrincipal::Local),
            AuthConfig::BearerToken(expected) => {
                let presented = bearer_token(headers).ok_or(ServerError::Unauthorized)?;
                if constant_time_eq(&hash_secret(expected), &hash_secret(presented)) {
                    Ok(AuthPrincipal::StaticBearer)
                } else {
                    Err(ServerError::Forbidden)
                }
            }
            AuthConfig::Forwarded(cfg) => {
                let Some(peer_ip) = peer.map(|peer| peer.ip()) else {
                    return Err(ServerError::Forbidden);
                };
                if !cfg.trusts(peer_ip) {
                    return Err(ServerError::Forbidden);
                }
                let value = headers
                    .get(cfg.user_header.as_str())
                    .ok_or(ServerError::Unauthorized)?
                    .to_str()
                    .map_err(|_| ServerError::Unauthorized)?;
                match &cfg.expected_user {
                    Some(expected) if value != expected => Err(ServerError::Forbidden),
                    _ => Ok(AuthPrincipal::Forwarded(value.to_string())),
                }
            }
            AuthConfig::Device(cfg) => {
                let (token, credential) = if let Some(token) = bearer_token(headers) {
                    (token, DeviceCredential::Bearer)
                } else if let Some(token) = cookie_value(headers, &cfg.cookie_name) {
                    (token, DeviceCredential::Cookie)
                } else {
                    return Err(ServerError::Unauthorized);
                };
                let device = self
                    .devices
                    .authenticate_device(&hash_secret(token), Utc::now())
                    .await?
                    .ok_or(ServerError::Forbidden)?;
                if device.owner_subject != cfg.owner_subject {
                    return Err(ServerError::Forbidden);
                }
                Ok(AuthPrincipal::Device { device, credential })
            }
        }
    }

    pub async fn pairing_code_creator(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<Option<Uuid>> {
        if matches!(
            &self.config,
            AuthConfig::Device(config)
                if config.allow_loopback_pairing
                    && peer.is_some_and(|peer| peer.ip().is_loopback())
                    && request_host_is_loopback(headers)
        ) {
            return Ok(None);
        }
        let count = self.devices.auth_device_count().await?;
        if count == 0 {
            if let AuthConfig::Device(cfg) = &self.config
                && let (Some(expected), Some(presented)) = (
                    cfg.bootstrap_token_hash.as_deref(),
                    headers
                        .get(BOOTSTRAP_HEADER)
                        .and_then(|value| value.to_str().ok()),
                )
                && constant_time_eq(expected, &hash_secret(presented))
            {
                return Ok(None);
            }
            return Err(ServerError::Forbidden);
        }

        match self.authenticate(headers, peer).await? {
            AuthPrincipal::Device { device, .. } => Ok(Some(device.id)),
            _ => Ok(None),
        }
    }

    pub fn device_config(&self) -> Option<&DeviceAuthConfig> {
        match &self.config {
            AuthConfig::Device(config) => Some(config),
            _ => None,
        }
    }

    pub fn validate_cookie_mutation(&self, headers: &HeaderMap) -> Result<()> {
        let Some(config) = self.device_config() else {
            return Ok(());
        };
        if bearer_token(headers).is_none()
            && cookie_value(headers, &config.cookie_name).is_some()
            && !cookie_origin_allowed(self, headers)
        {
            return Err(ServerError::Forbidden);
        }
        Ok(())
    }
}

pub async fn require_auth(
    State(auth): State<AuthContext>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0);
    match auth.authenticate(request.headers(), peer).await {
        Ok(principal) => {
            if matches!(
                principal,
                AuthPrincipal::Device {
                    credential: DeviceCredential::Cookie,
                    ..
                }
            ) && mutation_requires_csrf(request.method())
                && !cookie_origin_allowed(&auth, request.headers())
            {
                return ServerError::Forbidden.into_response();
            }
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(error) => error.into_response(),
    }
}

pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

pub fn new_device_token() -> Result<String> {
    Ok(format!("tmk_dev_{}", random_hex_32()?))
}

pub fn new_pairing_code() -> Result<String> {
    random_hex_32()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|item| item.trim().split_once('='))
        .find_map(|(key, value)| (key == name && !value.is_empty()).then_some(value))
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn random_hex_32() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|err| ServerError::Backend(format!("OS random source unavailable: {err}")))?;
    Ok(hex::encode(bytes))
}

fn mutation_requires_csrf(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn cookie_origin_allowed(auth: &AuthContext, headers: &HeaderMap) -> bool {
    let Some(config) = auth.device_config() else {
        return false;
    };
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let expected = config.allowed_origin.clone().or_else(|| {
        let host = headers.get(header::HOST)?.to_str().ok()?;
        Some(format!(
            "{}://{host}",
            if config.secure_cookie {
                "https"
            } else {
                "http"
            }
        ))
    });
    expected.as_deref().is_some_and(|expected| {
        constant_time_eq(expected.trim_end_matches('/'), origin.trim_end_matches('/'))
    })
}

fn request_host_is_loopback(headers: &HeaderMap) -> bool {
    let Some(authority) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let authority = authority.trim().to_ascii_lowercase();
    authority == "localhost"
        || authority.starts_with("localhost:")
        || authority == "127.0.0.1"
        || authority.starts_with("127.0.0.1:")
        || authority == "[::1]"
        || authority.starts_with("[::1]:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_pairing_codes_are_high_entropy_and_hashable() {
        let token = new_device_token().unwrap();
        let code = new_pairing_code().unwrap();
        assert!(token.starts_with("tmk_dev_"));
        assert!(token.len() >= 72);
        assert!(code.len() >= 48);
        assert_eq!(hash_secret(&token).len(), 64);
        assert_ne!(new_device_token().unwrap(), token);
        assert_ne!(new_pairing_code().unwrap(), code);
    }

    #[test]
    fn debug_output_redacts_auth_and_pairing_secrets() {
        let bearer = "bearer-secret-never-log";
        let bootstrap_hash = "bootstrap-secret-hash-never-log";
        let device_hash = "device-token-hash-never-log";
        let pairing_hash = "pairing-code-hash-never-log";
        let now = Utc::now();
        let output = format!(
            "{:?} {:?} {:?} {:?}",
            AuthConfig::BearerToken(bearer.to_string()),
            AuthConfig::Device(DeviceAuthConfig {
                bootstrap_token_hash: Some(bootstrap_hash.to_string()),
                ..DeviceAuthConfig::default()
            }),
            NewAuthDevice {
                id: Uuid::nil(),
                owner_subject: "owner".to_string(),
                name: "phone".to_string(),
                platform: "android".to_string(),
                token_hash: device_hash.to_string(),
                created_at: now,
            },
            NewPairingCode {
                id: Uuid::nil(),
                code_hash: pairing_hash.to_string(),
                created_at: now,
                expires_at: now,
                created_by_device_id: None,
            }
        );

        for secret in [bearer, bootstrap_hash, device_hash, pairing_hash] {
            assert!(!output.contains(secret), "debug output leaked {secret}");
        }
        assert!(output.matches("[REDACTED]").count() >= 4);
    }

    #[tokio::test]
    async fn pairing_code_cannot_be_redeemed_at_or_after_expiry() {
        let store = InMemoryAuthDeviceStore::default();
        let created_at = Utc::now();
        let expires_at = created_at + chrono::Duration::minutes(5);
        store
            .create_pairing_code(NewPairingCode {
                id: Uuid::new_v4(),
                code_hash: hash_secret("expired-code"),
                created_at,
                expires_at,
                created_by_device_id: None,
            })
            .await
            .unwrap();

        let result = store
            .consume_pairing_code(
                &hash_secret("expired-code"),
                NewAuthDevice {
                    id: Uuid::new_v4(),
                    owner_subject: "brian".to_string(),
                    name: "Too late".to_string(),
                    platform: "test".to_string(),
                    token_hash: hash_secret("unused-token"),
                    created_at: expires_at,
                },
                expires_at,
            )
            .await;

        assert!(matches!(result, Err(ServerError::Forbidden)));
        assert_eq!(store.auth_device_count().await.unwrap(), 0);
        assert!(store.auth_devices().await.unwrap().is_empty());
    }
}
