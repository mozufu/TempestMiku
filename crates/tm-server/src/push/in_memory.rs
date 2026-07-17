use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::Result;

use super::{
    EncryptedSecret, PushDeliveryLease, PushRegistrationMetadata, PushRegistrationRecord,
    PushRuntimeMetrics, PushStore,
};

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
