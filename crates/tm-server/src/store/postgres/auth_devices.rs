use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio_postgres::Row;
use uuid::Uuid;

use crate::{
    AuthDeviceRecord, AuthDeviceStore, NewAuthDevice, NewPairingCode, PairingCodeRecord, Result,
    ServerError,
};

use super::PostgresStore;

#[async_trait]
impl AuthDeviceStore for PostgresStore {
    async fn auth_device_count(&self) -> Result<usize> {
        let count: i64 = self
            .client
            .query_one("select count(*)::bigint from auth_devices", &[])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .get(0);
        Ok(count.max(0) as usize)
    }

    async fn authenticate_device(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AuthDeviceRecord>> {
        let row = self
            .client
            .query_opt(
                "update auth_devices
                    set last_seen_at = $2
                  where token_hash = $1 and revoked_at is null
                  returning id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at",
                &[&token_hash, &now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row.map(row_to_auth_device))
    }

    async fn create_pairing_code(&self, code: NewPairingCode) -> Result<PairingCodeRecord> {
        let row = self
            .client
            .query_one(
                "insert into pairing_codes
                    (id, code_hash, created_at, expires_at, consumed_at, created_by_device_id)
                 values ($1, $2, $3, $4, null, $5)
                 returning id, code_hash, created_at, expires_at, consumed_at, created_by_device_id",
                &[
                    &code.id,
                    &code.code_hash,
                    &code.created_at,
                    &code.expires_at,
                    &code.created_by_device_id,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(row_to_pairing_code(row))
    }

    async fn consume_pairing_code(
        &self,
        code_hash: &str,
        device: NewAuthDevice,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord> {
        let row = self
            .client
            .query_opt(
                "with consumed as (
                    update pairing_codes
                       set consumed_at = $2
                     where code_hash = $1
                       and consumed_at is null
                       and expires_at > $2
                     returning id
                 ), inserted as (
                    insert into auth_devices
                        (id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at)
                    select $3, $4, $5, $6, $7, $8, $8, null
                      from consumed
                    returning id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at
                 )
                 select id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at
                   from inserted",
                &[
                    &code_hash,
                    &now,
                    &device.id,
                    &device.owner_subject,
                    &device.name,
                    &device.platform,
                    &device.token_hash,
                    &device.created_at,
                ],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or(ServerError::Forbidden)?;
        Ok(row_to_auth_device(row))
    }

    async fn auth_devices(&self) -> Result<Vec<AuthDeviceRecord>> {
        let rows = self
            .client
            .query(
                "select id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at
                   from auth_devices
                  order by created_at desc",
                &[],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(rows.into_iter().map(row_to_auth_device).collect())
    }

    async fn revoke_auth_device(
        &self,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AuthDeviceRecord> {
        let row = self
            .client
            .query_opt(
                "update auth_devices
                    set revoked_at = coalesce(revoked_at, $2)
                  where id = $1
                  returning id, owner_subject, name, platform, token_hash, created_at, last_seen_at, revoked_at",
                &[&device_id, &now],
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .ok_or_else(|| ServerError::NotFound(format!("auth device {device_id}")))?;
        Ok(row_to_auth_device(row))
    }
}

fn row_to_auth_device(row: Row) -> AuthDeviceRecord {
    AuthDeviceRecord {
        id: row.get("id"),
        owner_subject: row.get("owner_subject"),
        name: row.get("name"),
        platform: row.get("platform"),
        token_hash: row.get("token_hash"),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
        revoked_at: row.get("revoked_at"),
    }
}

fn row_to_pairing_code(row: Row) -> PairingCodeRecord {
    PairingCodeRecord {
        id: row.get("id"),
        code_hash: row.get("code_hash"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        consumed_at: row.get("consumed_at"),
        created_by_device_id: row.get("created_by_device_id"),
    }
}
