use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Result, ServerError};

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
    pub(super) fn parse(value: &str) -> Result<Self> {
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
