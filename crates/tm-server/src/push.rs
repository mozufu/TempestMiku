use std::{collections::BTreeMap, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_postgres::{Config as PostgresConfig, NoTls};
use url::Url;
use uuid::Uuid;

use crate::{Result, ServerError};

const SECRET_VERSION: i16 = 1;
const MAX_DELIVERY_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct PushCipher {
    key: Arc<[u8; 32]>,
}

impl fmt::Debug for PushCipher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PushCipher").finish_non_exhaustive()
    }
}

impl PushCipher {
    pub fn from_base64(value: &str) -> Result<Self> {
        let decoded = STANDARD.decode(value.trim()).map_err(|_| {
            ServerError::InvalidRequest("TM_PUSH_ENCRYPTION_KEY must be base64-encoded".to_string())
        })?;
        let key: [u8; 32] = decoded.try_into().map_err(|_| {
            ServerError::InvalidRequest(
                "TM_PUSH_ENCRYPTION_KEY must decode to exactly 32 bytes".to_string(),
            )
        })?;
        Ok(Self { key: Arc::new(key) })
    }

    pub fn generate_for_tests() -> Self {
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).expect("test push key generation succeeds");
        Self { key: Arc::new(key) }
    }

    fn encrypt(&self, device_id: Uuid, provider: &str, secret: &str) -> Result<EncryptedSecret> {
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).map_err(|error| {
            ServerError::Store(format!("push nonce generation failed: {error}"))
        })?;
        let cipher = XChaCha20Poly1305::new(self.key.as_ref().into());
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: push_secret_aad(device_id, provider).as_bytes(),
                },
            )
            .map_err(|_| ServerError::Store("push registration encryption failed".to_string()))?;
        Ok(EncryptedSecret {
            ciphertext,
            nonce: nonce.to_vec(),
            version: SECRET_VERSION,
        })
    }

    fn decrypt(
        &self,
        device_id: Uuid,
        provider: &str,
        encrypted: &EncryptedSecret,
    ) -> Result<String> {
        if encrypted.version != SECRET_VERSION || encrypted.nonce.len() != 24 {
            return Err(ServerError::Store(
                "unsupported push registration secret envelope".to_string(),
            ));
        }
        let cipher = XChaCha20Poly1305::new(self.key.as_ref().into());
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(&encrypted.nonce),
                Payload {
                    msg: &encrypted.ciphertext,
                    aad: push_secret_aad(device_id, provider).as_bytes(),
                },
            )
            .map_err(|_| ServerError::Store("push registration decryption failed".to_string()))?;
        String::from_utf8(plaintext)
            .map_err(|_| ServerError::Store("push registration is not UTF-8".to_string()))
    }
}

fn push_secret_aad(device_id: Uuid, provider: &str) -> String {
    format!("tempestmiku.push.v1:{device_id}:{provider}")
}

#[derive(Clone)]
pub struct EncryptedSecret {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub version: i16,
}

impl fmt::Debug for EncryptedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedSecret")
            .field("ciphertext", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("version", &self.version)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PushRegistrationMetadata {
    pub device_id: Uuid,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct PushRegistrationRecord {
    pub metadata: PushRegistrationMetadata,
    pub encrypted_secret: EncryptedSecret,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PushRuntimeMetrics {
    pub queue_depth: i64,
    pub oldest_age_seconds: i64,
    pub retries: i64,
    pub failed_deliveries: i64,
    pub disabled_registrations: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PushMessageKind {
    ApprovalRequested,
    ApprovalResolved,
    SessionReady,
}

impl PushMessageKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "approval_requested" => Ok(Self::ApprovalRequested),
            "approval_resolved" => Ok(Self::ApprovalResolved),
            "session_ready" => Ok(Self::SessionReady),
            other => Err(ServerError::Store(format!(
                "unknown push message kind {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PushMessage {
    pub version: u8,
    pub delivery_id: Uuid,
    pub kind: PushMessageKind,
    pub session_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<i64>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PushDeliveryLease {
    pub message: PushMessage,
    pub device_id: Uuid,
    pub provider: String,
    pub encrypted_secret: EncryptedSecret,
    pub attempts: i32,
    pub lease_owner: Uuid,
    pub lease_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushProviderOutcome {
    Delivered,
    TransientFailure,
    PermanentRegistrationFailure,
}

#[derive(Debug, Clone)]
pub struct PushProviderResult {
    pub outcome: PushProviderOutcome,
    pub error: Option<String>,
}

impl PushProviderResult {
    pub fn delivered() -> Self {
        Self {
            outcome: PushProviderOutcome::Delivered,
            error: None,
        }
    }
}

#[async_trait]
pub trait PushProvider: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn deliver(&self, registration: &str, message: &PushMessage) -> PushProviderResult;
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnifiedPushRegistration {
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Clone)]
pub struct UnifiedPushProvider {
    client: reqwest::Client,
    allowed_origin: Url,
}

impl fmt::Debug for UnifiedPushProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnifiedPushProvider")
            .field("allowed_origin", &self.allowed_origin.as_str())
            .finish_non_exhaustive()
    }
}

impl UnifiedPushProvider {
    pub fn new(allowed_origin: &str) -> Result<Self> {
        Self::build(allowed_origin, false)
    }

    fn build(allowed_origin: &str, allow_http: bool) -> Result<Self> {
        let mut allowed_origin = Url::parse(allowed_origin).map_err(|_| {
            ServerError::InvalidRequest(
                "TM_UNIFIED_PUSH_ENDPOINT_ORIGIN must be an absolute URL".to_string(),
            )
        })?;
        if (!allow_http && allowed_origin.scheme() != "https")
            || allowed_origin.cannot_be_a_base()
            || allowed_origin.host_str().is_none()
            || !allowed_origin.username().is_empty()
            || allowed_origin.password().is_some()
            || allowed_origin.query().is_some()
            || allowed_origin.fragment().is_some()
        {
            return Err(ServerError::InvalidRequest(
                "TM_UNIFIED_PUSH_ENDPOINT_ORIGIN must be an HTTPS origin without credentials, path, query, or fragment"
                    .to_string(),
            ));
        }
        if allowed_origin.path() != "/" {
            return Err(ServerError::InvalidRequest(
                "TM_UNIFIED_PUSH_ENDPOINT_ORIGIN must not contain a path".to_string(),
            ));
        }
        allowed_origin.set_path("");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ServerError::InvalidRequest(error.to_string()))?;
        Ok(Self {
            client,
            allowed_origin,
        })
    }

    fn parse_registration(&self, raw: &str) -> Result<(Url, Vec<u8>, Vec<u8>)> {
        let registration: UnifiedPushRegistration = serde_json::from_str(raw).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush registration envelope".to_string())
        })?;
        let endpoint = Url::parse(&registration.endpoint).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush endpoint URL".to_string())
        })?;
        if endpoint.origin() != self.allowed_origin.origin()
            || endpoint.scheme() != self.allowed_origin.scheme()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ServerError::Policy(
                "UnifiedPush endpoint is outside the configured origin".to_string(),
            ));
        }
        let public_key = URL_SAFE_NO_PAD.decode(registration.p256dh).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush p256dh key".to_string())
        })?;
        let auth = URL_SAFE_NO_PAD.decode(registration.auth).map_err(|_| {
            ServerError::InvalidRequest("invalid UnifiedPush auth secret".to_string())
        })?;
        if public_key.len() != 65 || public_key.first() != Some(&4) || auth.len() != 16 {
            return Err(ServerError::InvalidRequest(
                "invalid UnifiedPush Web Push key material".to_string(),
            ));
        }
        Ok((endpoint, public_key, auth))
    }

    fn invalid_registration(error: impl fmt::Display) -> PushProviderResult {
        PushProviderResult {
            outcome: PushProviderOutcome::PermanentRegistrationFailure,
            error: Some(error.to_string()),
        }
    }

    fn transient(error: impl fmt::Display) -> PushProviderResult {
        PushProviderResult {
            outcome: PushProviderOutcome::TransientFailure,
            error: Some(error.to_string()),
        }
    }
}

#[async_trait]
impl PushProvider for UnifiedPushProvider {
    fn name(&self) -> &str {
        "unifiedpush"
    }

    async fn deliver(&self, registration: &str, message: &PushMessage) -> PushProviderResult {
        let (endpoint, public_key, auth) = match self.parse_registration(registration) {
            Ok(parsed) => parsed,
            Err(error) => return Self::invalid_registration(error),
        };
        let payload = match serde_json::to_vec(message) {
            Ok(payload) => payload,
            Err(_) => return Self::transient("UnifiedPush payload serialization failed"),
        };
        let encrypted = match ece::encrypt(&public_key, &auth, &payload) {
            Ok(encrypted) => encrypted,
            Err(_) => {
                return Self::invalid_registration("UnifiedPush registration encryption failed");
            }
        };
        let ttl = (message.expires_at - Utc::now())
            .num_seconds()
            .clamp(0, 3600);
        let response = match self
            .client
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
            .header("TTL", ttl)
            .header("Urgency", "high")
            .body(encrypted)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return Self::transient("UnifiedPush delivery transport failed"),
        };
        let status = response.status();
        if status.is_success() {
            PushProviderResult::delivered()
        } else if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            Self::invalid_registration(format!("UnifiedPush endpoint returned {status}"))
        } else {
            Self::transient(format!("UnifiedPush endpoint returned {status}"))
        }
    }
}

#[async_trait]
pub trait PushStore: Send + Sync + 'static {
    async fn upsert_registration(
        &self,
        device_id: Uuid,
        provider: &str,
        encrypted: EncryptedSecret,
        now: DateTime<Utc>,
    ) -> Result<PushRegistrationMetadata>;
    async fn disable_registration(
        &self,
        device_id: Uuid,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<PushRegistrationMetadata>>;
    async fn materialize_deliveries(&self, now: DateTime<Utc>, limit: i64) -> Result<usize>;
    async fn claim_next_delivery(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: chrono::Duration,
    ) -> Result<Option<PushDeliveryLease>>;
    async fn complete_delivery(&self, lease: &PushDeliveryLease, now: DateTime<Utc>) -> Result<()>;
    async fn retry_delivery(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        available_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<()>;
    async fn fail_delivery(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()>;
    async fn fail_delivery_and_disable_registration(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()>;
    async fn runtime_metrics(&self, now: DateTime<Utc>) -> Result<PushRuntimeMetrics>;
}

#[derive(Clone)]
pub struct PushService {
    store: Arc<dyn PushStore>,
    provider: Arc<dyn PushProvider>,
    cipher: PushCipher,
}

impl fmt::Debug for PushService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PushService")
            .field("provider", &self.provider.name())
            .finish_non_exhaustive()
    }
}

impl PushService {
    pub fn new(
        store: Arc<dyn PushStore>,
        provider: Arc<dyn PushProvider>,
        cipher: PushCipher,
    ) -> Self {
        Self {
            store,
            provider,
            cipher,
        }
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub async fn register(
        &self,
        device_id: Uuid,
        provider: &str,
        secret: &str,
    ) -> Result<PushRegistrationMetadata> {
        if provider != self.provider.name() {
            return Err(ServerError::InvalidRequest(format!(
                "push provider {provider} is not configured"
            )));
        }
        let encrypted = self.cipher.encrypt(device_id, provider, secret)?;
        self.store
            .upsert_registration(device_id, provider, encrypted, Utc::now())
            .await
    }

    pub async fn unregister(&self, device_id: Uuid) -> Result<()> {
        self.store
            .disable_registration(device_id, None, Utc::now())
            .await?;
        Ok(())
    }

    pub async fn runtime_metrics(&self) -> Result<PushRuntimeMetrics> {
        self.store.runtime_metrics(Utc::now()).await
    }

    pub async fn tick(&self, owner_id: Uuid) -> Result<usize> {
        let now = Utc::now();
        self.store.materialize_deliveries(now, 64).await?;
        let mut handled = 0;
        for _ in 0..32 {
            let Some(lease) = self
                .store
                .claim_next_delivery(owner_id, Utc::now(), chrono::Duration::seconds(60))
                .await?
            else {
                break;
            };
            let registration =
                match self
                    .cipher
                    .decrypt(lease.device_id, &lease.provider, &lease.encrypted_secret)
                {
                    Ok(registration) => registration,
                    Err(error) => {
                        self.store
                            .fail_delivery_and_disable_registration(
                                &lease,
                                &error.to_string(),
                                Utc::now(),
                            )
                            .await?;
                        handled += 1;
                        continue;
                    }
                };
            let result = self.provider.deliver(&registration, &lease.message).await;
            let error = result.error.as_deref().unwrap_or("push provider failure");
            match result.outcome {
                PushProviderOutcome::Delivered => {
                    self.store.complete_delivery(&lease, Utc::now()).await?;
                }
                PushProviderOutcome::PermanentRegistrationFailure => {
                    self.store
                        .fail_delivery_and_disable_registration(&lease, error, Utc::now())
                        .await?;
                }
                PushProviderOutcome::TransientFailure => {
                    if lease.attempts >= MAX_DELIVERY_ATTEMPTS
                        || Utc::now() >= lease.message.expires_at
                    {
                        self.store.fail_delivery(&lease, error, Utc::now()).await?;
                    } else {
                        let delay = retry_delay(lease.attempts);
                        let available_at = Utc::now()
                            + chrono::Duration::from_std(delay)
                                .expect("push retry delay fits chrono duration");
                        self.store
                            .retry_delivery(&lease, error, available_at, Utc::now())
                            .await?;
                    }
                }
            }
            handled += 1;
        }
        Ok(handled)
    }
}

fn retry_delay(attempts: i32) -> Duration {
    match attempts {
        ..=1 => Duration::from_secs(1),
        2 => Duration::from_secs(5),
        3 => Duration::from_secs(30),
        4 => Duration::from_secs(120),
        _ => Duration::from_secs(300),
    }
}

pub struct PostgresPushStore {
    client: tokio_postgres::Client,
}

impl PostgresPushStore {
    pub async fn connect(dsn: &str) -> Result<Self> {
        let config = dsn
            .parse::<PostgresConfig>()
            .map_err(|error| ServerError::Store(error.to_string()))?;
        let (client, connection) = config
            .connect(NoTls)
            .await
            .map_err(|error| ServerError::Store(error.to_string()))?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                let error = tm_memory::redact_dream_text(&error.to_string()).text;
                tracing::error!(%error, "push Postgres connection failed");
            }
        });
        Ok(Self { client })
    }
}

#[async_trait]
impl PushStore for PostgresPushStore {
    async fn upsert_registration(
        &self,
        device_id: Uuid,
        provider: &str,
        encrypted: EncryptedSecret,
        now: DateTime<Utc>,
    ) -> Result<PushRegistrationMetadata> {
        let row = self
            .client
            .query_opt(
                "insert into device_push_registrations
                (device_id, provider, secret_ciphertext, secret_nonce, secret_version,
                 created_at, updated_at, disabled_at, last_error)
             select id, $2, $3, $4, $5, $6, $6, null, null
               from auth_devices where id = $1 and revoked_at is null
             on conflict (device_id) do update
               set provider = excluded.provider,
                   secret_ciphertext = excluded.secret_ciphertext,
                   secret_nonce = excluded.secret_nonce,
                   secret_version = excluded.secret_version,
                   updated_at = excluded.updated_at,
                   disabled_at = null,
                   last_error = null
             returning device_id, provider, created_at, updated_at, disabled_at",
                &[
                    &device_id,
                    &provider,
                    &encrypted.ciphertext,
                    &encrypted.nonce,
                    &encrypted.version,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        row.map(row_to_registration)
            .ok_or_else(|| ServerError::NotFound(format!("active device {device_id}")))
    }

    async fn disable_registration(
        &self,
        device_id: Uuid,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<PushRegistrationMetadata>> {
        let row = self.client.query_opt(
            "with disabled as (
                update device_push_registrations
                   set disabled_at = coalesce(disabled_at, $2), updated_at = $2, last_error = $3
                 where device_id = $1
                 returning device_id, provider, created_at, updated_at, disabled_at
             ), failed as (
                update push_deliveries
                   set status = 'failed', failed_at = $2, updated_at = $2,
                       locked_at = null, lease_owner = null, last_error = coalesce($3, 'registration disabled')
                 where device_id = $1 and status in ('pending', 'claimed')
             ) select * from disabled",
            &[&device_id, &now, &error],
        ).await.map_err(store_error)?;
        Ok(row.map(row_to_registration))
    }

    async fn materialize_deliveries(&self, now: DateTime<Utc>, limit: i64) -> Result<usize> {
        self.client
            .execute(
                "update push_deliveries delivery
                set status = 'failed', failed_at = $1, updated_at = $1,
                    locked_at = null, lease_owner = null,
                    last_error = 'device or registration is inactive'
              where delivery.status in ('pending', 'claimed')
                and not exists (
                    select 1 from device_push_registrations registration
                    join auth_devices device on device.id = registration.device_id
                    where registration.device_id = delivery.device_id
                      and registration.disabled_at is null and device.revoked_at is null
                )",
                &[&now],
            )
            .await
            .map_err(store_error)?;
        let rows = self.client.query(
            "with candidates as (
                select approval.id as approval_id, approval.session_id, approval.expires_at,
                       registration.device_id
                  from approval_requests approval
                  join device_push_registrations registration on registration.disabled_at is null
                  join auth_devices device on device.id = registration.device_id and device.revoked_at is null
                 where approval.status = 'pending'
                   and approval.request_event_seq is not null
                   and approval.expires_at > $1
                   and not exists (
                       select 1 from push_deliveries existing
                        where existing.approval_id = approval.id
                          and existing.device_id = registration.device_id
                          and existing.kind = 'approval_requested'
                   )
                 order by approval.created_at, registration.device_id
                 limit $2
             )
             insert into push_deliveries
                (id, approval_id, device_id, kind, session_id, event_seq, expires_at,
                 status, attempts, available_at,
                 locked_at, lease_owner, lease_epoch, delivered_at, failed_at,
                 last_error, created_at, updated_at)
             select gen_random_uuid(), approval_id, device_id, 'approval_requested', session_id,
                    null, expires_at, 'pending', 0, $1, null, null, 0, null, null, null, $1, $1
               from candidates
             on conflict (approval_id, device_id, kind) where approval_id is not null do nothing
             returning id",
            &[&now, &limit],
        ).await.map_err(store_error)?;
        let resolved = self.client.query(
            "with candidates as (
                select requested.approval_id, requested.device_id
                  from push_deliveries requested
                  join approval_requests approval on approval.id = requested.approval_id
                  join device_push_registrations registration
                    on registration.device_id = requested.device_id and registration.disabled_at is null
                  join auth_devices device on device.id = requested.device_id and device.revoked_at is null
                 where requested.kind = 'approval_requested'
                   and requested.status = 'delivered'
                   and approval.status <> 'pending'
                   and not exists (
                       select 1 from push_deliveries existing
                        where existing.approval_id = requested.approval_id
                          and existing.device_id = requested.device_id
                          and existing.kind = 'approval_resolved'
                   )
                 order by approval.resolved_at, requested.device_id
                 limit $2
             )
             insert into push_deliveries
                (id, approval_id, device_id, kind, session_id, event_seq, expires_at,
                 status, attempts, available_at,
                 locked_at, lease_owner, lease_epoch, delivered_at, failed_at,
                 last_error, created_at, updated_at)
             select gen_random_uuid(), approval_id, device_id, 'approval_resolved',
                    approval.session_id, null, approval.expires_at, 'pending', 0,
                    $1, null, null, 0, null, null, null, $1, $1
               from candidates
               join approval_requests approval on approval.id = candidates.approval_id
             on conflict (approval_id, device_id, kind) where approval_id is not null do nothing
             returning id",
            &[&now, &limit],
        ).await.map_err(store_error)?;
        let session_ready = self.client.query(
            "with candidates as (
                select event.session_id, event.seq as event_seq, registration.device_id,
                       event.created_at + interval '1 hour' as expires_at
                  from session_events event
                  join sessions session on session.id = event.session_id and session.status <> 'ended'
                  join device_push_registrations registration
                    on registration.disabled_at is null and registration.created_at <= event.created_at
                  join auth_devices device
                    on device.id = registration.device_id and device.revoked_at is null
                 where event.event_type = 'final'
                   and event.created_at + interval '1 hour' > $1
                   and not exists (
                       select 1 from push_deliveries existing
                        where existing.session_id = event.session_id
                          and existing.event_seq = event.seq
                          and existing.device_id = registration.device_id
                          and existing.kind = 'session_ready'
                   )
                 order by event.created_at, registration.device_id
                 limit $2
             )
             insert into push_deliveries
                (id, approval_id, device_id, kind, session_id, event_seq, expires_at,
                 status, attempts, available_at, locked_at, lease_owner, lease_epoch,
                 delivered_at, failed_at, last_error, created_at, updated_at)
             select gen_random_uuid(), null, device_id, 'session_ready', session_id, event_seq,
                    expires_at, 'pending', 0, $1, null, null, 0, null, null, null, $1, $1
               from candidates
             on conflict (session_id, event_seq, device_id, kind) where event_seq is not null
             do nothing
             returning id",
            &[&now, &limit],
        ).await.map_err(store_error)?;
        Ok(rows.len() + resolved.len() + session_ready.len())
    }

    async fn claim_next_delivery(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_timeout: chrono::Duration,
    ) -> Result<Option<PushDeliveryLease>> {
        let stale_before = now - lease_timeout;
        let row = self
            .client
            .query_opt(
                "with selected as (
                select delivery.id
                  from push_deliveries delivery
                 where (delivery.status = 'pending' and delivery.available_at <= $2)
                    or (delivery.status = 'claimed' and delivery.locked_at <= $3)
                 order by delivery.available_at, delivery.created_at
                 for update skip locked limit 1
             )
             update push_deliveries delivery
                set status = 'claimed', attempts = delivery.attempts + 1,
                    locked_at = $2, lease_owner = $1, lease_epoch = delivery.lease_epoch + 1,
                    updated_at = $2, failed_at = null, last_error = null
               from selected, device_push_registrations registration, auth_devices device
              where delivery.id = selected.id
                and registration.device_id = delivery.device_id
                and registration.disabled_at is null
                and device.id = delivery.device_id
                and device.revoked_at is null
             returning delivery.id, delivery.device_id, delivery.kind, delivery.attempts,
                       delivery.lease_owner, delivery.lease_epoch, delivery.session_id,
                       delivery.approval_id, delivery.event_seq, delivery.expires_at,
                       registration.provider,
                       registration.secret_ciphertext, registration.secret_nonce,
                       registration.secret_version",
                &[&owner_id, &now, &stale_before],
            )
            .await
            .map_err(store_error)?;
        row.map(row_to_delivery).transpose()
    }

    async fn complete_delivery(&self, lease: &PushDeliveryLease, now: DateTime<Utc>) -> Result<()> {
        let changed = self
            .client
            .execute(
                "update push_deliveries
                set status = 'delivered', delivered_at = $4, updated_at = $4,
                    locked_at = null, lease_owner = null
              where id = $1 and status = 'claimed' and lease_owner = $2 and lease_epoch = $3",
                &[
                    &lease.message.delivery_id,
                    &lease.lease_owner,
                    &lease.lease_epoch,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        ensure_one_delivery_changed(changed, lease)
    }

    async fn retry_delivery(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        available_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let changed = self
            .client
            .execute(
                "update push_deliveries
                set status = 'pending', available_at = $4, updated_at = $5,
                    locked_at = null, lease_owner = null, failed_at = $5, last_error = $6
              where id = $1 and status = 'claimed' and lease_owner = $2 and lease_epoch = $3",
                &[
                    &lease.message.delivery_id,
                    &lease.lease_owner,
                    &lease.lease_epoch,
                    &available_at,
                    &now,
                    &redact_error(error),
                ],
            )
            .await
            .map_err(store_error)?;
        ensure_one_delivery_changed(changed, lease)
    }

    async fn fail_delivery(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let changed = self
            .client
            .execute(
                "update push_deliveries
                set status = 'failed', failed_at = $4, updated_at = $4,
                    locked_at = null, lease_owner = null, last_error = $5
              where id = $1 and status = 'claimed' and lease_owner = $2 and lease_epoch = $3",
                &[
                    &lease.message.delivery_id,
                    &lease.lease_owner,
                    &lease.lease_epoch,
                    &now,
                    &redact_error(error),
                ],
            )
            .await
            .map_err(store_error)?;
        ensure_one_delivery_changed(changed, lease)
    }

    async fn fail_delivery_and_disable_registration(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let error = redact_error(error);
        let changed = self
            .client
            .execute(
                "with failed as (
                update push_deliveries
                   set status = 'failed', failed_at = $4, updated_at = $4,
                       locked_at = null, lease_owner = null, last_error = $5
                 where id = $1 and status = 'claimed' and lease_owner = $2 and lease_epoch = $3
                 returning device_id
             ) update device_push_registrations registration
                  set disabled_at = coalesce(disabled_at, $4), updated_at = $4, last_error = $5
                 from failed where registration.device_id = failed.device_id",
                &[
                    &lease.message.delivery_id,
                    &lease.lease_owner,
                    &lease.lease_epoch,
                    &now,
                    &error,
                ],
            )
            .await
            .map_err(store_error)?;
        ensure_one_delivery_changed(changed, lease)
    }

    async fn runtime_metrics(&self, now: DateTime<Utc>) -> Result<PushRuntimeMetrics> {
        let row = self
            .client
            .query_one(
                "select
                (select count(*)::bigint from push_deliveries
                  where status in ('pending', 'claimed')) as queue_depth,
                (select min(created_at) from push_deliveries
                  where status in ('pending', 'claimed')) as oldest,
                (select coalesce(sum(greatest(attempts - 1, 0)), 0)::bigint
                   from push_deliveries) as retries,
                (select count(*)::bigint from push_deliveries
                  where status = 'failed') as failed,
                (select count(*)::bigint from device_push_registrations
                  where disabled_at is not null) as disabled",
                &[],
            )
            .await
            .map_err(store_error)?;
        let oldest: Option<DateTime<Utc>> = row.get("oldest");
        Ok(PushRuntimeMetrics {
            queue_depth: row.get("queue_depth"),
            oldest_age_seconds: oldest
                .map(|value| now.signed_duration_since(value).num_seconds().max(0))
                .unwrap_or(0),
            retries: row.get("retries"),
            failed_deliveries: row.get("failed"),
            disabled_registrations: row.get("disabled"),
        })
    }
}

fn ensure_one_delivery_changed(changed: u64, lease: &PushDeliveryLease) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(ServerError::Conflict(format!(
            "push delivery {} lease is stale",
            lease.message.delivery_id
        )))
    }
}

fn row_to_registration(row: tokio_postgres::Row) -> PushRegistrationMetadata {
    PushRegistrationMetadata {
        device_id: row.get("device_id"),
        provider: row.get("provider"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        disabled_at: row.get("disabled_at"),
    }
}

fn row_to_delivery(row: tokio_postgres::Row) -> Result<PushDeliveryLease> {
    Ok(PushDeliveryLease {
        message: PushMessage {
            version: 1,
            delivery_id: row.get("id"),
            kind: PushMessageKind::parse(row.get("kind"))?,
            session_id: row.get("session_id"),
            approval_id: row.get("approval_id"),
            event_seq: row.get("event_seq"),
            expires_at: row.get("expires_at"),
        },
        device_id: row.get("device_id"),
        provider: row.get("provider"),
        encrypted_secret: EncryptedSecret {
            ciphertext: row.get("secret_ciphertext"),
            nonce: row.get("secret_nonce"),
            version: row.get("secret_version"),
        },
        attempts: row.get("attempts"),
        lease_owner: row.get("lease_owner"),
        lease_epoch: row.get("lease_epoch"),
    })
}

fn redact_error(error: &str) -> String {
    tm_memory::redact_dream_text(error)
        .text
        .chars()
        .take(512)
        .collect()
}

fn store_error(error: tokio_postgres::Error) -> ServerError {
    ServerError::Store(error.to_string())
}

#[derive(Default)]
pub struct FakePushProvider {
    deliveries: Mutex<Vec<(String, PushMessage)>>,
    outcomes: Mutex<Vec<PushProviderResult>>,
}

impl fmt::Debug for FakePushProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakePushProvider")
            .field("delivery_count", &self.deliveries.lock().len())
            .field("queued_outcome_count", &self.outcomes.lock().len())
            .finish()
    }
}

impl FakePushProvider {
    pub fn deliveries(&self) -> Vec<(String, PushMessage)> {
        self.deliveries.lock().clone()
    }

    pub fn queue_outcome(&self, outcome: PushProviderResult) {
        self.outcomes.lock().push(outcome);
    }
}

#[async_trait]
impl PushProvider for FakePushProvider {
    fn name(&self) -> &str {
        "fake"
    }

    async fn deliver(&self, registration: &str, message: &PushMessage) -> PushProviderResult {
        self.deliveries
            .lock()
            .push((registration.to_string(), message.clone()));
        let mut outcomes = self.outcomes.lock();
        if outcomes.is_empty() {
            PushProviderResult::delivered()
        } else {
            outcomes.remove(0)
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemoryPushStore {
    inner: Mutex<InMemoryPushState>,
}

#[derive(Debug, Default)]
struct InMemoryPushState {
    registrations: BTreeMap<Uuid, PushRegistrationRecord>,
}

#[async_trait]
impl PushStore for InMemoryPushStore {
    async fn upsert_registration(
        &self,
        device_id: Uuid,
        provider: &str,
        encrypted: EncryptedSecret,
        now: DateTime<Utc>,
    ) -> Result<PushRegistrationMetadata> {
        let mut inner = self.inner.lock();
        let created_at = inner
            .registrations
            .get(&device_id)
            .map_or(now, |record| record.metadata.created_at);
        let metadata = PushRegistrationMetadata {
            device_id,
            provider: provider.to_string(),
            created_at,
            updated_at: now,
            disabled_at: None,
        };
        inner.registrations.insert(
            device_id,
            PushRegistrationRecord {
                metadata: metadata.clone(),
                encrypted_secret: encrypted,
            },
        );
        Ok(metadata)
    }

    async fn disable_registration(
        &self,
        device_id: Uuid,
        _error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<PushRegistrationMetadata>> {
        let mut inner = self.inner.lock();
        Ok(inner.registrations.get_mut(&device_id).map(|record| {
            record.metadata.disabled_at.get_or_insert(now);
            record.metadata.updated_at = now;
            record.metadata.clone()
        }))
    }

    async fn materialize_deliveries(&self, _now: DateTime<Utc>, _limit: i64) -> Result<usize> {
        Ok(0)
    }

    async fn claim_next_delivery(
        &self,
        _owner_id: Uuid,
        _now: DateTime<Utc>,
        _lease_timeout: chrono::Duration,
    ) -> Result<Option<PushDeliveryLease>> {
        Ok(None)
    }

    async fn complete_delivery(
        &self,
        _lease: &PushDeliveryLease,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        Ok(())
    }

    async fn retry_delivery(
        &self,
        _lease: &PushDeliveryLease,
        _error: &str,
        _available_at: DateTime<Utc>,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        Ok(())
    }

    async fn fail_delivery(
        &self,
        _lease: &PushDeliveryLease,
        _error: &str,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        Ok(())
    }

    async fn fail_delivery_and_disable_registration(
        &self,
        lease: &PushDeliveryLease,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.disable_registration(lease.device_id, Some(error), now)
            .await?;
        Ok(())
    }

    async fn runtime_metrics(&self, _now: DateTime<Utc>) -> Result<PushRuntimeMetrics> {
        let inner = self.inner.lock();
        Ok(PushRuntimeMetrics {
            disabled_registrations: inner
                .registrations
                .values()
                .filter(|record| record.metadata.disabled_at.is_some())
                .count() as i64,
            ..PushRuntimeMetrics::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        http::{HeaderMap, StatusCode},
        routing::post,
    };

    #[derive(Debug)]
    struct ScriptedPushStore {
        lease: Mutex<Option<PushDeliveryLease>>,
        completed: Mutex<usize>,
        retried: Mutex<usize>,
        failed: Mutex<usize>,
        disabled: Mutex<usize>,
    }

    impl ScriptedPushStore {
        fn new(lease: PushDeliveryLease) -> Self {
            Self {
                lease: Mutex::new(Some(lease)),
                completed: Mutex::new(0),
                retried: Mutex::new(0),
                failed: Mutex::new(0),
                disabled: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl PushStore for ScriptedPushStore {
        async fn upsert_registration(
            &self,
            _device_id: Uuid,
            _provider: &str,
            _encrypted: EncryptedSecret,
            _now: DateTime<Utc>,
        ) -> Result<PushRegistrationMetadata> {
            panic!("unused")
        }

        async fn disable_registration(
            &self,
            _device_id: Uuid,
            _error: Option<&str>,
            _now: DateTime<Utc>,
        ) -> Result<Option<PushRegistrationMetadata>> {
            panic!("unused")
        }

        async fn materialize_deliveries(&self, _now: DateTime<Utc>, _limit: i64) -> Result<usize> {
            Ok(0)
        }

        async fn claim_next_delivery(
            &self,
            _owner_id: Uuid,
            _now: DateTime<Utc>,
            _lease_timeout: chrono::Duration,
        ) -> Result<Option<PushDeliveryLease>> {
            Ok(self.lease.lock().take())
        }

        async fn complete_delivery(
            &self,
            _lease: &PushDeliveryLease,
            _now: DateTime<Utc>,
        ) -> Result<()> {
            *self.completed.lock() += 1;
            Ok(())
        }

        async fn retry_delivery(
            &self,
            _lease: &PushDeliveryLease,
            _error: &str,
            _available_at: DateTime<Utc>,
            _now: DateTime<Utc>,
        ) -> Result<()> {
            *self.retried.lock() += 1;
            Ok(())
        }

        async fn fail_delivery(
            &self,
            _lease: &PushDeliveryLease,
            _error: &str,
            _now: DateTime<Utc>,
        ) -> Result<()> {
            *self.failed.lock() += 1;
            Ok(())
        }

        async fn fail_delivery_and_disable_registration(
            &self,
            _lease: &PushDeliveryLease,
            _error: &str,
            _now: DateTime<Utc>,
        ) -> Result<()> {
            *self.disabled.lock() += 1;
            Ok(())
        }

        async fn runtime_metrics(&self, _now: DateTime<Utc>) -> Result<PushRuntimeMetrics> {
            Ok(PushRuntimeMetrics::default())
        }
    }

    fn scripted_lease(cipher: &PushCipher, attempts: i32) -> PushDeliveryLease {
        let device_id = Uuid::new_v4();
        PushDeliveryLease {
            message: PushMessage {
                version: 1,
                delivery_id: Uuid::new_v4(),
                kind: PushMessageKind::ApprovalRequested,
                session_id: Uuid::new_v4(),
                approval_id: Some(Uuid::new_v4()),
                event_seq: None,
                expires_at: Utc::now() + chrono::Duration::minutes(5),
            },
            device_id,
            provider: "fake".to_string(),
            encrypted_secret: cipher
                .encrypt(device_id, "fake", "opaque-registration")
                .unwrap(),
            attempts,
            lease_owner: Uuid::new_v4(),
            lease_epoch: 1,
        }
    }

    #[test]
    fn registration_secrets_are_bound_to_device_and_provider() {
        let cipher = PushCipher::generate_for_tests();
        let device_id = Uuid::new_v4();
        let encrypted = cipher.encrypt(device_id, "fake", "opaque-token").unwrap();
        let debug = format!("{encrypted:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(&hex::encode(&encrypted.ciphertext)));
        assert_eq!(
            cipher.decrypt(device_id, "fake", &encrypted).unwrap(),
            "opaque-token"
        );
        assert!(cipher.decrypt(Uuid::new_v4(), "fake", &encrypted).is_err());
        assert!(cipher.decrypt(device_id, "other", &encrypted).is_err());
    }

    #[test]
    fn encryption_key_parser_fails_closed() {
        assert!(PushCipher::from_base64("not base64").is_err());
        assert!(PushCipher::from_base64(&STANDARD.encode([7_u8; 31])).is_err());
        assert!(PushCipher::from_base64(&STANDARD.encode([7_u8; 32])).is_ok());
    }

    #[test]
    fn provider_payload_contains_only_routing_identifiers() {
        let message = PushMessage {
            version: 1,
            delivery_id: Uuid::new_v4(),
            kind: PushMessageKind::ApprovalRequested,
            session_id: Uuid::new_v4(),
            approval_id: Some(Uuid::new_v4()),
            event_seq: None,
            expires_at: Utc::now(),
        };
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        for forbidden in ["action", "scope", "token", "transcript", "credential"] {
            assert!(!value.to_string().contains(forbidden));
        }

        let session_message = PushMessage {
            version: 1,
            delivery_id: Uuid::new_v4(),
            kind: PushMessageKind::SessionReady,
            session_id: Uuid::new_v4(),
            approval_id: None,
            event_seq: Some(42),
            expires_at: Utc::now(),
        };
        let value = serde_json::to_value(session_message).unwrap();
        assert_eq!(value["kind"], "session_ready");
        assert_eq!(value["eventSeq"], 42);
        assert!(value.get("approvalId").is_none());
    }

    #[test]
    fn unified_push_origin_and_registration_fail_closed() {
        assert!(UnifiedPushProvider::new("http://push.example.test").is_err());
        assert!(UnifiedPushProvider::new("https://user@push.example.test").is_err());
        assert!(UnifiedPushProvider::new("https://push.example.test/path").is_err());

        let provider = UnifiedPushProvider::new("https://push.example.test").unwrap();
        let valid_keys = serde_json::json!({
            "endpoint": "https://push.example.test/up-secret",
            "p256dh": "BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8",
            "auth": "xS03Fi5ErfTNH_l9WHE9Ig"
        });
        assert!(provider.parse_registration(&valid_keys.to_string()).is_ok());

        let wrong_origin = serde_json::json!({
            "endpoint": "https://internal.example.test/up-secret",
            "p256dh": valid_keys["p256dh"],
            "auth": valid_keys["auth"]
        });
        assert!(
            provider
                .parse_registration(&wrong_origin.to_string())
                .is_err()
        );
    }

    #[tokio::test]
    async fn unified_push_posts_encrypted_routing_payload() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
        let sent_tx = Arc::new(Mutex::new(Some(sent_tx)));
        let app = Router::new().route(
            "/up-secret",
            post({
                let sent_tx = Arc::clone(&sent_tx);
                move |headers: HeaderMap, body: Bytes| async move {
                    if let Some(sender) = sent_tx.lock().take() {
                        let _ = sender.send((headers, body));
                    }
                    StatusCode::OK
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = UnifiedPushProvider::build(&origin, true).unwrap();
        let message = PushMessage {
            version: 1,
            delivery_id: Uuid::new_v4(),
            kind: PushMessageKind::ApprovalRequested,
            session_id: Uuid::new_v4(),
            approval_id: Some(Uuid::new_v4()),
            event_seq: None,
            expires_at: Utc::now() + chrono::Duration::minutes(5),
        };
        let registration = serde_json::json!({
            "endpoint": format!("{origin}/up-secret"),
            "p256dh": "BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8",
            "auth": "xS03Fi5ErfTNH_l9WHE9Ig"
        });

        let result = provider.deliver(&registration.to_string(), &message).await;
        assert_eq!(result.outcome, PushProviderOutcome::Delivered);
        let (headers, body) = sent_rx.await.unwrap();
        assert_eq!(headers[reqwest::header::CONTENT_ENCODING], "aes128gcm");
        assert_eq!(
            headers[reqwest::header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(headers["urgency"], "high");
        assert!(!body.is_empty());
        let approval_id = message.approval_id.expect("approval route id").to_string();
        assert!(
            !body
                .windows(approval_id.len())
                .any(|part| part == approval_id.as_bytes())
        );
        server.abort();
    }

    #[tokio::test]
    async fn unified_push_transport_errors_do_not_expose_endpoint_capabilities() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let provider = UnifiedPushProvider::build(&origin, true).unwrap();
        let message = PushMessage {
            version: 1,
            delivery_id: Uuid::new_v4(),
            kind: PushMessageKind::ApprovalRequested,
            session_id: Uuid::new_v4(),
            approval_id: Some(Uuid::new_v4()),
            event_seq: None,
            expires_at: Utc::now() + chrono::Duration::minutes(5),
        };
        let registration = serde_json::json!({
            "endpoint": format!("{origin}/up-secret-capability"),
            "p256dh": "BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8",
            "auth": "xS03Fi5ErfTNH_l9WHE9Ig"
        });

        let result = provider.deliver(&registration.to_string(), &message).await;
        assert_eq!(result.outcome, PushProviderOutcome::TransientFailure);
        assert_eq!(
            result.error.as_deref(),
            Some("UnifiedPush delivery transport failed")
        );
        assert!(!result.error.unwrap().contains("up-secret-capability"));
    }

    #[tokio::test]
    async fn fake_provider_delivery_uses_decrypted_registration_and_completes() {
        let cipher = PushCipher::generate_for_tests();
        let store = Arc::new(ScriptedPushStore::new(scripted_lease(&cipher, 1)));
        let provider = Arc::new(FakePushProvider::default());
        let service = PushService::new(store.clone(), provider.clone(), cipher);

        assert_eq!(service.tick(Uuid::new_v4()).await.unwrap(), 1);
        assert_eq!(*store.completed.lock(), 1);
        assert_eq!(provider.deliveries()[0].0, "opaque-registration");
        assert!(!format!("{provider:?}").contains("opaque-registration"));
    }

    #[tokio::test]
    async fn transient_failure_retries_but_permanent_failure_disables_registration() {
        let cipher = PushCipher::generate_for_tests();
        let transient_store = Arc::new(ScriptedPushStore::new(scripted_lease(&cipher, 1)));
        let transient_provider = Arc::new(FakePushProvider::default());
        transient_provider.queue_outcome(PushProviderResult {
            outcome: PushProviderOutcome::TransientFailure,
            error: Some("temporary outage".to_string()),
        });
        PushService::new(transient_store.clone(), transient_provider, cipher.clone())
            .tick(Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(*transient_store.retried.lock(), 1);
        assert_eq!(*transient_store.disabled.lock(), 0);

        let permanent_store = Arc::new(ScriptedPushStore::new(scripted_lease(&cipher, 1)));
        let permanent_provider = Arc::new(FakePushProvider::default());
        permanent_provider.queue_outcome(PushProviderResult {
            outcome: PushProviderOutcome::PermanentRegistrationFailure,
            error: Some("invalid registration".to_string()),
        });
        PushService::new(permanent_store.clone(), permanent_provider, cipher)
            .tick(Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(*permanent_store.disabled.lock(), 1);
        assert_eq!(*permanent_store.retried.lock(), 0);
    }
}
