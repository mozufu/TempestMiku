use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio_postgres::{Config as PostgresConfig, NoTls};
use uuid::Uuid;

use crate::{Result, ServerError};

use super::{
    EncryptedSecret, PushDeliveryLease, PushMessage, PushMessageKind, PushRegistrationMetadata,
    PushRuntimeMetrics, PushStore,
};

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
