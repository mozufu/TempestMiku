use std::{fmt, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use tm_drive::{
    DRIVE_METADATA_SCHEMA_VERSION, DriveCorrectionRecord, DriveEntry, DriveEntryId, DriveError,
    DriveLinkRecord, DriveMetadataSnapshot, DriveMetadataStore, DriveMoveCommit,
    DriveOrganizerRunId, OrganizerProposal, OrganizerProposalCommit, OrganizerRun,
    initial_record_version,
};
use tokio::sync::Mutex;
use tokio_postgres::NoTls;
use uuid::Uuid;

mod persistence;
mod records;
mod transactions;

use persistence::{
    insert_correction, replace_entry_children, write_entry, write_link, write_proposal, write_run,
};
use records::{
    json_value, map_entry_write_error, next_version, query_record_opt, query_records,
    require_version, store_error, to_i64, validate_replacement, version_error,
};

#[derive(Clone)]
pub struct PostgresDriveMetadataStore {
    client: Arc<Mutex<tokio_postgres::Client>>,
}

impl fmt::Debug for PostgresDriveMetadataStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresDriveMetadataStore")
            .finish_non_exhaustive()
    }
}

impl PostgresDriveMetadataStore {
    pub async fn connect(dsn: &str) -> tm_drive::Result<Self> {
        let (client, connection) = tokio_postgres::connect(dsn, NoTls)
            .await
            .map_err(store_error)?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                let err = tm_memory::redact_dream_text(&err.to_string()).text;
                tracing::error!(%err, "postgres drive metadata connection failed");
            }
        });
        Ok(Self {
            client: Arc::new(Mutex::new(client)),
        })
    }
}

#[async_trait]
impl DriveMetadataStore for PostgresDriveMetadataStore {
    async fn entry(&self, id: DriveEntryId) -> tm_drive::Result<Option<DriveEntry>> {
        let client = self.client.lock().await;
        query_record_opt(
            &*client,
            "select record_json from drive_entries where id=$1",
            &id,
        )
        .await
    }

    async fn entry_by_path(&self, path: &str) -> tm_drive::Result<Option<DriveEntry>> {
        let client = self.client.lock().await;
        query_record_opt(
            &*client,
            "select record_json from drive_entries where path=$1",
            &path,
        )
        .await
    }

    async fn entries(&self) -> tm_drive::Result<Vec<DriveEntry>> {
        let client = self.client.lock().await;
        query_records(
            &*client,
            "select record_json from drive_entries order by created_at, id",
        )
        .await
    }

    async fn insert_entry(&self, mut entry: DriveEntry) -> tm_drive::Result<DriveEntry> {
        entry.version = initial_record_version();
        let mut client = self.client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        if let Err(err) = write_entry(&transaction, None, &entry).await {
            return Err(map_entry_write_error(err, &entry.path));
        }
        replace_entry_children(&transaction, &entry).await?;
        transaction.commit().await.map_err(store_error)?;
        Ok(entry)
    }

    async fn compare_and_swap_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
        mut replacement: DriveEntry,
    ) -> tm_drive::Result<DriveEntry> {
        validate_replacement("entry", id, replacement.id)?;
        replacement.version = next_version("entry", id, expected_version)?;
        let mut client = self.client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        let updated = match write_entry(&transaction, Some(expected_version), &replacement).await {
            Ok(updated) => updated,
            Err(err) => return Err(map_entry_write_error(err, &replacement.path)),
        };
        if updated == 0 {
            return Err(version_error(
                &transaction,
                "entry",
                "drive_entries",
                "id",
                &id,
                expected_version,
            )
            .await?);
        }
        replace_entry_children(&transaction, &replacement).await?;
        transaction.commit().await.map_err(store_error)?;
        Ok(replacement)
    }

    async fn remove_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
    ) -> tm_drive::Result<DriveEntry> {
        let mut client = self.client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        let entry: DriveEntry = query_record_opt(
            &transaction,
            "select record_json from drive_entries where id=$1",
            &id,
        )
        .await?
        .ok_or_else(|| DriveError::NotFound(format!("drive entry {id}")))?;
        require_version("entry", id, expected_version, entry.version)?;
        let record = json_value(&entry)?;
        transaction
            .execute(
                "insert into drive_entry_tombstones(id,version,path,entry_json,deleted_at)
                 values($1,$2,$3,$4,$5)
                 on conflict(id) do update set version=excluded.version,path=excluded.path,
                    entry_json=excluded.entry_json,deleted_at=excluded.deleted_at",
                &[
                    &entry.id,
                    &to_i64("entry version", entry.version)?,
                    &entry.path,
                    &record,
                    &Utc::now(),
                ],
            )
            .await
            .map_err(store_error)?;
        let deleted = transaction
            .execute(
                "delete from drive_entries where id=$1 and version=$2",
                &[&id, &to_i64("expected entry version", expected_version)?],
            )
            .await
            .map_err(store_error)?;
        if deleted == 0 {
            return Err(DriveError::Conflict {
                entity: "entry",
                id: id.to_string(),
                expected: expected_version,
                actual: entry.version,
            });
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(entry)
    }

    async fn commit_move(&self, commit: DriveMoveCommit) -> tm_drive::Result<DriveEntry> {
        self.commit_move_transaction(commit).await
    }

    async fn proposals(&self) -> tm_drive::Result<Vec<OrganizerProposal>> {
        let client = self.client.lock().await;
        query_records(
            &*client,
            "select record_json from drive_proposals order by created_at, id",
        )
        .await
    }

    async fn insert_proposal(
        &self,
        mut proposal: OrganizerProposal,
    ) -> tm_drive::Result<OrganizerProposal> {
        proposal.version = initial_record_version();
        let client = self.client.lock().await;
        write_proposal(&*client, None, &proposal).await?;
        Ok(proposal)
    }

    async fn compare_and_swap_proposal(
        &self,
        id: Uuid,
        expected_version: u64,
        mut replacement: OrganizerProposal,
    ) -> tm_drive::Result<OrganizerProposal> {
        validate_replacement("proposal", id, replacement.id)?;
        replacement.version = next_version("proposal", id, expected_version)?;
        let client = self.client.lock().await;
        if write_proposal(&*client, Some(expected_version), &replacement).await? == 0 {
            return Err(version_error(
                &*client,
                "proposal",
                "drive_proposals",
                "id",
                &id,
                expected_version,
            )
            .await?);
        }
        Ok(replacement)
    }

    async fn commit_organizer_proposal(
        &self,
        commit: OrganizerProposalCommit,
    ) -> tm_drive::Result<OrganizerProposal> {
        self.commit_organizer_proposal_transaction(commit).await
    }

    async fn organizer_runs(&self) -> tm_drive::Result<Vec<OrganizerRun>> {
        let client = self.client.lock().await;
        query_records(
            &*client,
            "select record_json from drive_organizer_runs order by created_at, id",
        )
        .await
    }

    async fn insert_organizer_run(&self, mut run: OrganizerRun) -> tm_drive::Result<OrganizerRun> {
        run.version = initial_record_version();
        let client = self.client.lock().await;
        write_run(&*client, None, &run).await?;
        Ok(run)
    }

    async fn compare_and_swap_organizer_run(
        &self,
        id: DriveOrganizerRunId,
        expected_version: u64,
        mut replacement: OrganizerRun,
    ) -> tm_drive::Result<OrganizerRun> {
        validate_replacement("organizer run", id, replacement.id)?;
        replacement.version = next_version("organizer run", id, expected_version)?;
        let client = self.client.lock().await;
        if write_run(&*client, Some(expected_version), &replacement).await? == 0 {
            return Err(version_error(
                &*client,
                "organizer run",
                "drive_organizer_runs",
                "id",
                &id,
                expected_version,
            )
            .await?);
        }
        Ok(replacement)
    }

    async fn links(&self) -> tm_drive::Result<Vec<DriveLinkRecord>> {
        let client = self.client.lock().await;
        query_records(
            &*client,
            "select record_json from drive_links order by created_at, alias",
        )
        .await
    }

    async fn link(&self, alias: &str) -> tm_drive::Result<Option<DriveLinkRecord>> {
        let client = self.client.lock().await;
        query_record_opt(
            &*client,
            "select record_json from drive_links where alias=$1",
            &alias,
        )
        .await
    }

    async fn insert_link(&self, mut link: DriveLinkRecord) -> tm_drive::Result<DriveLinkRecord> {
        link.version = initial_record_version();
        let client = self.client.lock().await;
        write_link(&*client, None, &link).await?;
        Ok(link)
    }

    async fn compare_and_swap_link(
        &self,
        alias: &str,
        expected_version: u64,
        mut replacement: DriveLinkRecord,
    ) -> tm_drive::Result<DriveLinkRecord> {
        if replacement.alias != alias {
            return Err(DriveError::InvalidArgs(format!(
                "replacement link alias {} does not match {alias}",
                replacement.alias
            )));
        }
        replacement.version = next_version("link", alias, expected_version)?;
        let client = self.client.lock().await;
        if write_link(&*client, Some(expected_version), &replacement).await? == 0 {
            return Err(version_error(
                &*client,
                "link",
                "drive_links",
                "alias",
                &alias,
                expected_version,
            )
            .await?);
        }
        Ok(replacement)
    }

    async fn append_correction(
        &self,
        mut correction: DriveCorrectionRecord,
    ) -> tm_drive::Result<DriveCorrectionRecord> {
        correction.version = initial_record_version();
        let client = self.client.lock().await;
        insert_correction(&*client, &correction).await?;
        Ok(correction)
    }

    async fn snapshot(&self) -> tm_drive::Result<DriveMetadataSnapshot> {
        let corrections = {
            let client = self.client.lock().await;
            query_records(
                &*client,
                "select record_json from drive_corrections order by created_at, id",
            )
            .await?
        };
        Ok(DriveMetadataSnapshot {
            schema_version: DRIVE_METADATA_SCHEMA_VERSION,
            entries: self.entries().await?,
            proposals: self.proposals().await?,
            organizer_runs: self.organizer_runs().await?,
            links: self.links().await?,
            corrections,
        })
    }
}
