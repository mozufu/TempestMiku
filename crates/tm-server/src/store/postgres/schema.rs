use sha2::{Digest, Sha256};

use crate::{Result, ServerError};

use super::PostgresStore;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "base",
        sql: include_str!("../../../migrations/0001_base.sql"),
    },
    Migration {
        version: 2,
        name: "device_auth",
        sql: include_str!("../../../migrations/0002_device_auth.sql"),
    },
    Migration {
        version: 3,
        name: "durable_work_leases",
        sql: include_str!("../../../migrations/0003_durable_work_leases.sql"),
    },
    Migration {
        version: 4,
        name: "session_authority_turns",
        sql: include_str!("../../../migrations/0004_session_authority_turns.sql"),
    },
    Migration {
        version: 5,
        name: "durable_approvals",
        sql: include_str!("../../../migrations/0005_durable_approvals.sql"),
    },
    Migration {
        version: 6,
        name: "durable_drive_metadata",
        sql: include_str!("../../../migrations/0006_durable_drive_metadata.sql"),
    },
    Migration {
        version: 7,
        name: "auth_device_owner",
        sql: include_str!("../../../migrations/0007_auth_device_owner.sql"),
    },
    Migration {
        version: 8,
        name: "turn_heartbeats",
        sql: include_str!("../../../migrations/0008_turn_heartbeats.sql"),
    },
];

impl PostgresStore {
    pub(super) async fn ensure_schema(&mut self) -> Result<()> {
        self.client
            .query_one("select pg_advisory_lock($1)", &[&Self::SCHEMA_LOCK_ID])
            .await
            .map_err(store_error)?;

        let migration_result = self.run_migrations().await;
        let unlock_result = self
            .client
            .query_one("select pg_advisory_unlock($1)", &[&Self::SCHEMA_LOCK_ID])
            .await
            .map(|_| ())
            .map_err(store_error);

        match (migration_result, unlock_result) {
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn run_migrations(&mut self) -> Result<()> {
        self.client
            .batch_execute(
                "create table if not exists schema_migrations(
                    version bigint primary key,
                    name text not null,
                    checksum text not null,
                    applied_at timestamptz not null default now()
                )",
            )
            .await
            .map_err(store_error)?;

        for migration in MIGRATIONS {
            let checksum = hex::encode(Sha256::digest(migration.sql.as_bytes()));
            let applied = self
                .client
                .query_opt(
                    "select name, checksum from schema_migrations where version = $1",
                    &[&migration.version],
                )
                .await
                .map_err(store_error)?;
            if let Some(row) = applied {
                let stored_name: String = row.get("name");
                let stored_checksum: String = row.get("checksum");
                if stored_name != migration.name || stored_checksum != checksum {
                    return Err(ServerError::Store(format!(
                        "migration {} ({}) checksum mismatch; database has {} ({})",
                        migration.version, migration.name, stored_name, stored_checksum
                    )));
                }
                continue;
            }

            let transaction = self.client.transaction().await.map_err(store_error)?;
            transaction
                .batch_execute(migration.sql)
                .await
                .map_err(store_error)?;
            transaction
                .execute(
                    "insert into schema_migrations(version, name, checksum) values ($1, $2, $3)",
                    &[&migration.version, &migration.name, &checksum],
                )
                .await
                .map_err(store_error)?;
            transaction.commit().await.map_err(store_error)?;
        }
        Ok(())
    }
}

fn store_error(error: tokio_postgres::Error) -> ServerError {
    ServerError::Store(error.to_string())
}
