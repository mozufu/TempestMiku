use std::{fmt, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tm_drive::{
    DRIVE_METADATA_SCHEMA_VERSION, DriveCorrectionRecord, DriveEntry, DriveEntryId, DriveError,
    DriveLinkRecord, DriveMetadataSnapshot, DriveMetadataStore, DriveMoveCommit,
    DriveOrganizerRunId, OrganizerProposal, OrganizerProposalCommit, OrganizerRun, ProposalStatus,
    initial_record_version,
};
use tokio::sync::Mutex;
use tokio_postgres::{GenericClient, NoTls, error::SqlState};
use uuid::Uuid;

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
        let source_id = commit.source.replacement.id;
        let mut client = self.client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        let current: DriveEntry = query_record_opt(
            &transaction,
            "select record_json from drive_entries where id=$1 for update",
            &source_id,
        )
        .await?
        .ok_or_else(|| DriveError::NotFound(format!("drive entry {source_id}")))?;
        require_version(
            "entry",
            source_id,
            commit.source.expected_version,
            current.version,
        )?;
        validate_move_correction(&current, &commit.source.replacement, &commit.correction)?;

        let overwritten = if let Some(target) = commit.overwrite {
            if target.id == source_id {
                return Err(DriveError::InvalidArgs(
                    "move overwrite target must differ from source".to_string(),
                ));
            }
            let entry: DriveEntry = query_record_opt(
                &transaction,
                "select record_json from drive_entries where id=$1 for update",
                &target.id,
            )
            .await?
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {}", target.id)))?;
            require_version("entry", target.id, target.expected_version, entry.version)?;
            if entry.path != commit.source.replacement.path {
                return Err(DriveError::InvalidArgs(
                    "overwrite target no longer occupies the move destination".to_string(),
                ));
            }
            Some(entry)
        } else {
            None
        };
        if let Some(occupant) = query_record_opt::<_, DriveEntry, _>(
            &transaction,
            "select record_json from drive_entries where path=$1 for update",
            &commit.source.replacement.path,
        )
        .await?
            && occupant.id != source_id
            && overwritten
                .as_ref()
                .is_none_or(|entry| entry.id != occupant.id)
        {
            return Err(DriveError::Collision(
                commit.source.replacement.path.clone(),
            ));
        }

        let mut replacement = commit.source.replacement;
        replacement.version = next_version("entry", source_id, commit.source.expected_version)?;
        let mut correction = commit.correction;
        correction.version = initial_record_version();

        if let Some(overwritten) = overwritten {
            tombstone_and_delete_entry(&transaction, &overwritten).await?;
        }
        let updated = match write_entry(
            &transaction,
            Some(commit.source.expected_version),
            &replacement,
        )
        .await
        {
            Ok(updated) => updated,
            Err(err) => return Err(map_entry_write_error(err, &replacement.path)),
        };
        if updated == 0 {
            return Err(version_error(
                &transaction,
                "entry",
                "drive_entries",
                "id",
                &source_id,
                commit.source.expected_version,
            )
            .await?);
        }
        replace_entry_children(&transaction, &replacement).await?;
        insert_correction(&transaction, &correction).await?;
        transaction.commit().await.map_err(store_error)?;
        Ok(replacement)
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
        if matches!(
            &commit.replacement.status,
            ProposalStatus::Pending | ProposalStatus::Approved
        ) {
            return Err(DriveError::InvalidArgs(
                "organizer proposal commit requires a terminal status".to_string(),
            ));
        }
        if commit.entry_update.is_none() && commit.correction.is_some() {
            return Err(DriveError::InvalidArgs(
                "organizer correction requires an entry update".to_string(),
            ));
        }

        let proposal_id = commit.replacement.id;
        let mut client = self.client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        let current_proposal: OrganizerProposal = query_record_opt(
            &transaction,
            "select record_json from drive_proposals where id=$1 for update",
            &proposal_id,
        )
        .await?
        .ok_or_else(|| DriveError::NotFound(format!("organizer proposal {proposal_id}")))?;
        require_version(
            "proposal",
            proposal_id,
            commit.expected_proposal_version,
            current_proposal.version,
        )?;
        if !matches!(
            &current_proposal.status,
            ProposalStatus::Pending | ProposalStatus::Approved
        ) {
            return Ok(current_proposal);
        }
        if current_proposal.entry_id != commit.replacement.entry_id {
            return Err(DriveError::InvalidArgs(
                "organizer proposal replacement changed entry id".to_string(),
            ));
        }

        let entry_update = if let Some(mut update) = commit.entry_update {
            let entry_id = update.replacement.id;
            if entry_id != current_proposal.entry_id {
                return Err(DriveError::InvalidArgs(format!(
                    "organizer entry {entry_id} does not match proposal entry {}",
                    current_proposal.entry_id
                )));
            }
            let current_entry: DriveEntry = query_record_opt(
                &transaction,
                "select record_json from drive_entries where id=$1 for update",
                &entry_id,
            )
            .await?
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {entry_id}")))?;
            require_version(
                "entry",
                entry_id,
                update.expected_version,
                current_entry.version,
            )?;
            if let Some(occupant) = query_record_opt::<_, DriveEntry, _>(
                &transaction,
                "select record_json from drive_entries where path=$1 for update",
                &update.replacement.path,
            )
            .await?
                && occupant.id != entry_id
            {
                return Err(DriveError::Collision(update.replacement.path));
            }
            if let Some(correction) = commit.correction.as_ref() {
                validate_move_correction(&current_entry, &update.replacement, correction)?;
            }
            update.replacement.version = next_version("entry", entry_id, update.expected_version)?;
            Some(update)
        } else {
            None
        };
        let mut replacement_proposal = commit.replacement;
        replacement_proposal.version =
            next_version("proposal", proposal_id, commit.expected_proposal_version)?;
        let correction = commit.correction.map(|mut correction| {
            correction.version = initial_record_version();
            correction
        });

        if let Some(update) = entry_update {
            let updated = match write_entry(
                &transaction,
                Some(update.expected_version),
                &update.replacement,
            )
            .await
            {
                Ok(updated) => updated,
                Err(err) => return Err(map_entry_write_error(err, &update.replacement.path)),
            };
            if updated == 0 {
                return Err(version_error(
                    &transaction,
                    "entry",
                    "drive_entries",
                    "id",
                    &update.replacement.id,
                    update.expected_version,
                )
                .await?);
            }
            replace_entry_children(&transaction, &update.replacement).await?;
        }
        if let Some(correction) = correction.as_ref() {
            insert_correction(&transaction, correction).await?;
        }
        if write_proposal(
            &transaction,
            Some(commit.expected_proposal_version),
            &replacement_proposal,
        )
        .await?
            == 0
        {
            return Err(version_error(
                &transaction,
                "proposal",
                "drive_proposals",
                "id",
                &proposal_id,
                commit.expected_proposal_version,
            )
            .await?);
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(replacement_proposal)
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

fn validate_move_correction(
    current: &DriveEntry,
    replacement: &DriveEntry,
    correction: &DriveCorrectionRecord,
) -> tm_drive::Result<()> {
    if correction.from != current.path || correction.to != replacement.path {
        return Err(DriveError::InvalidArgs(
            "drive correction does not match move source and destination".to_string(),
        ));
    }
    Ok(())
}

async fn insert_correction<C: GenericClient + Sync>(
    client: &C,
    correction: &DriveCorrectionRecord,
) -> tm_drive::Result<()> {
    let record = json_value(correction)?;
    client
        .execute(
            "insert into drive_corrections(id,version,from_path,to_path,created_at,record_json)
             values($1,$2,$3,$4,$5,$6)",
            &[
                &correction.id,
                &to_i64("correction version", correction.version)?,
                &correction.from,
                &correction.to,
                &correction.created_at,
                &record,
            ],
        )
        .await
        .map_err(store_error)?;
    Ok(())
}

async fn tombstone_and_delete_entry<C: GenericClient + Sync>(
    client: &C,
    entry: &DriveEntry,
) -> tm_drive::Result<()> {
    let record = json_value(entry)?;
    client
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
    let deleted = client
        .execute(
            "delete from drive_entries where id=$1 and version=$2",
            &[&entry.id, &to_i64("entry version", entry.version)?],
        )
        .await
        .map_err(store_error)?;
    if deleted == 0 {
        return Err(DriveError::Conflict {
            entity: "entry",
            id: entry.id.to_string(),
            expected: entry.version,
            actual: entry.version,
        });
    }
    Ok(())
}

async fn write_entry<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    entry: &DriveEntry,
) -> Result<u64, tokio_postgres::Error> {
    let provenance = serde_json::to_value(&entry.provenance).unwrap();
    let entities = serde_json::to_value(&entry.entities).unwrap();
    let dates = serde_json::to_value(&entry.dates).unwrap();
    let amounts = serde_json::to_value(&entry.amounts).unwrap();
    let record = serde_json::to_value(entry).unwrap();
    let values: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[
        &entry.id,
        &i64::try_from(entry.version).unwrap(),
        &entry.path,
        &entry.uri,
        &entry.blob_uri,
        &entry.content_hash,
        &entry.mime,
        &i64::try_from(entry.size_bytes).unwrap(),
        &entry.title,
        &entry.doc_kind,
        &entry.project,
        &entry.source_uri,
        &provenance,
        &entry.summary,
        &enum_label(&entry.status).unwrap(),
        &entry.created_at,
        &entry.updated_at,
        &entities,
        &dates,
        &amounts,
        &entry.embedding,
        &record,
    ];
    if let Some(expected) = expected {
        let mut update_values = Vec::with_capacity(values.len() + 1);
        update_values.push(&entry.id as &(dyn tokio_postgres::types::ToSql + Sync));
        let expected = i64::try_from(expected).unwrap();
        update_values.push(&expected);
        update_values.extend_from_slice(&values[1..]);
        client
            .execute(
                "update drive_entries set version=$3,path=$4,uri=$5,blob_uri=$6,content_hash=$7,
             mime=$8,size_bytes=$9,title=$10,doc_kind=$11,project=$12,source_uri=$13,
             provenance_json=$14,summary=$15,status=$16,created_at=$17,updated_at=$18,
             entities_json=$19,dates_json=$20,amounts_json=$21,embedding=$22,record_json=$23
             where id=$1 and version=$2",
                &update_values,
            )
            .await
    } else {
        client.execute(
            "insert into drive_entries(id,version,path,uri,blob_uri,content_hash,mime,size_bytes,
             title,doc_kind,project,source_uri,provenance_json,summary,status,created_at,updated_at,
             entities_json,dates_json,amounts_json,embedding,record_json)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)",
            values,
        ).await
    }
}

async fn replace_entry_children<C: GenericClient + Sync>(
    client: &C,
    entry: &DriveEntry,
) -> tm_drive::Result<()> {
    client
        .execute(
            "delete from drive_attributes where entry_id=$1",
            &[&entry.id],
        )
        .await
        .map_err(store_error)?;
    for (index, attribute) in entry.attributes.iter().enumerate() {
        let evidence = attribute.evidence.as_ref().map(json_value).transpose()?;
        client
            .execute(
                "insert into drive_attributes(entry_id,idx,key,value,confidence,evidence_json,
             extractor,source_uri,session_id,event_seq,content_hash)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
                &[
                    &entry.id,
                    &i32::try_from(index)
                        .map_err(|_| DriveError::Store("too many drive attributes".to_string()))?,
                    &attribute.key,
                    &attribute.value,
                    &attribute.confidence,
                    &evidence,
                    &attribute.extractor,
                    &attribute.source_uri,
                    &attribute.session_id,
                    &attribute.event_seq,
                    &attribute.content_hash,
                ],
            )
            .await
            .map_err(store_error)?;
    }
    client
        .execute("delete from drive_tags where entry_id=$1", &[&entry.id])
        .await
        .map_err(store_error)?;
    for tag in &entry.tags {
        client
            .execute(
                "insert into drive_tags(entry_id,tag) values($1,$2) on conflict do nothing",
                &[&entry.id, tag],
            )
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

async fn write_proposal<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    proposal: &OrganizerProposal,
) -> tm_drive::Result<u64> {
    let proposed_tags = json_value(&proposal.proposed_tags)?;
    let evidence = json_value(&proposal.evidence)?;
    let replay = json_value(&proposal.replay_metadata)?;
    let record = json_value(proposal)?;
    if let Some(expected) = expected {
        client.execute(
            "update drive_proposals set version=$3,action=$4,
             entry_id=case when exists(select 1 from drive_entries where id=$5) then $5 else null end,
             entry_id_snapshot=$5,source_path=$6,proposed_path=$7,proposed_tags=$8,
             proposed_doc_kind=$9,proposed_project=$10,evidence_json=$11,confidence=$12,
             policy_decision=$13,approval_id=$14,status=$15,source_run_id=$16,
             replay_metadata=$17,created_at=$18,updated_at=$19,record_json=$20
             where id=$1 and version=$2",
            &[&proposal.id,&to_i64("expected proposal version", expected)?,
              &to_i64("proposal version", proposal.version)?,&enum_label(&proposal.action)?,
              &proposal.entry_id,&proposal.source_path,&proposal.proposed_path,&proposed_tags,
              &proposal.proposed_doc_kind,&proposal.proposed_project,&evidence,&proposal.confidence,
              &enum_label(&proposal.policy_decision)?,&proposal.approval_id,
              &enum_label(&proposal.status)?,&proposal.source_run_id,&replay,
              &proposal.created_at,&proposal.updated_at,&record],
        ).await.map_err(store_error)
    } else {
        client.execute(
            "insert into drive_proposals(id,version,action,entry_id,entry_id_snapshot,source_path,
             proposed_path,proposed_tags,proposed_doc_kind,proposed_project,evidence_json,confidence,
             policy_decision,approval_id,status,source_run_id,replay_metadata,created_at,updated_at,record_json)
             values($1,$2,$3,$4,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
            &[&proposal.id,&to_i64("proposal version", proposal.version)?,
              &enum_label(&proposal.action)?,&proposal.entry_id,&proposal.source_path,
              &proposal.proposed_path,&proposed_tags,&proposal.proposed_doc_kind,
              &proposal.proposed_project,&evidence,&proposal.confidence,
              &enum_label(&proposal.policy_decision)?,&proposal.approval_id,
              &enum_label(&proposal.status)?,&proposal.source_run_id,&replay,
              &proposal.created_at,&proposal.updated_at,&record],
        ).await.map_err(store_error)
    }
}

async fn write_run<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    run: &OrganizerRun,
) -> tm_drive::Result<u64> {
    let proposal_ids = json_value(&run.proposal_ids)?;
    let record = json_value(run)?;
    let attempts = i32::try_from(run.attempts)
        .map_err(|_| DriveError::Store("organizer attempts overflow".to_string()))?;
    if let Some(expected) = expected {
        client
            .execute(
                "update drive_organizer_runs set version=$3,trigger=$4,status=$5,attempts=$6,
             proposal_ids=$7,created_at=$8,available_at=$9,locked_at=$10,completed_at=$11,
             last_error=$12,record_json=$13 where id=$1 and version=$2",
                &[
                    &run.id,
                    &to_i64("expected organizer run version", expected)?,
                    &to_i64("organizer run version", run.version)?,
                    &run.trigger,
                    &enum_label(&run.status)?,
                    &attempts,
                    &proposal_ids,
                    &run.created_at,
                    &run.available_at,
                    &run.locked_at,
                    &run.completed_at,
                    &run.last_error,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    } else {
        client
            .execute(
                "insert into drive_organizer_runs(id,version,trigger,status,attempts,proposal_ids,
             created_at,available_at,locked_at,completed_at,last_error,record_json)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
                &[
                    &run.id,
                    &to_i64("organizer run version", run.version)?,
                    &run.trigger,
                    &enum_label(&run.status)?,
                    &attempts,
                    &proposal_ids,
                    &run.created_at,
                    &run.available_at,
                    &run.locked_at,
                    &run.completed_at,
                    &run.last_error,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    }
}

async fn write_link<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    link: &DriveLinkRecord,
) -> tm_drive::Result<u64> {
    let metadata = json_value(&link.metadata)?;
    let record = json_value(link)?;
    if let Some(expected) = expected {
        client
            .execute(
                "update drive_links set version=$3,canonical_root=$4,mode=$5,linked_uri=$6,
             memory_scope=$7,project=$8,status=$9,metadata_json=$10,created_at=$11,
             updated_at=$12,revoked_at=$13,record_json=$14 where alias=$1 and version=$2",
                &[
                    &link.alias,
                    &to_i64("expected link version", expected)?,
                    &to_i64("link version", link.version)?,
                    &link.canonical_root,
                    &link.mode,
                    &link.linked_uri,
                    &link.memory_scope,
                    &link.project,
                    &enum_label(&link.status)?,
                    &metadata,
                    &link.created_at,
                    &link.updated_at,
                    &link.revoked_at,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    } else {
        client
            .execute(
                "insert into drive_links(alias,version,canonical_root,mode,linked_uri,memory_scope,
             project,status,metadata_json,created_at,updated_at,revoked_at,record_json)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
                &[
                    &link.alias,
                    &to_i64("link version", link.version)?,
                    &link.canonical_root,
                    &link.mode,
                    &link.linked_uri,
                    &link.memory_scope,
                    &link.project,
                    &enum_label(&link.status)?,
                    &metadata,
                    &link.created_at,
                    &link.updated_at,
                    &link.revoked_at,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    }
}

async fn query_records<C, T>(client: &C, query: &str) -> tm_drive::Result<Vec<T>>
where
    C: GenericClient + Sync,
    T: DeserializeOwned,
{
    client
        .query(query, &[])
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|row| from_json(row.get("record_json")))
        .collect()
}

async fn query_record_opt<C, T, I>(client: &C, query: &str, id: &I) -> tm_drive::Result<Option<T>>
where
    C: GenericClient + Sync,
    T: DeserializeOwned,
    I: tokio_postgres::types::ToSql + Sync,
{
    client
        .query_opt(query, &[id])
        .await
        .map_err(store_error)?
        .map(|row| from_json(row.get("record_json")))
        .transpose()
}

async fn version_error<C, I>(
    client: &C,
    entity: &'static str,
    table: &str,
    key: &str,
    id: &I,
    expected: u64,
) -> tm_drive::Result<DriveError>
where
    C: GenericClient + Sync,
    I: tokio_postgres::types::ToSql + Sync + ToString,
{
    let query = format!("select version from {table} where {key}=$1");
    let row = client.query_opt(&query, &[id]).await.map_err(store_error)?;
    Ok(match row {
        Some(row) => DriveError::Conflict {
            entity,
            id: id.to_string(),
            expected,
            actual: u64::try_from(row.get::<_, i64>("version"))
                .map_err(|_| DriveError::Store("negative drive record version".to_string()))?,
        },
        None => DriveError::NotFound(format!("{entity} {}", id.to_string())),
    })
}

fn validate_replacement(
    entity: &'static str,
    expected_id: impl ToString,
    actual_id: impl ToString,
) -> tm_drive::Result<()> {
    if expected_id.to_string() == actual_id.to_string() {
        Ok(())
    } else {
        Err(DriveError::InvalidArgs(format!(
            "replacement {entity} id {} does not match {}",
            actual_id.to_string(),
            expected_id.to_string()
        )))
    }
}

fn require_version(
    entity: &'static str,
    id: impl ToString,
    expected: u64,
    actual: u64,
) -> tm_drive::Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(DriveError::Conflict {
            entity,
            id: id.to_string(),
            expected,
            actual,
        })
    }
}

fn next_version(entity: &'static str, id: impl ToString, version: u64) -> tm_drive::Result<u64> {
    version.checked_add(1).ok_or_else(|| {
        DriveError::Store(format!(
            "drive {entity} {} version overflow",
            id.to_string()
        ))
    })
}

fn to_i64(label: &str, value: u64) -> tm_drive::Result<i64> {
    i64::try_from(value).map_err(|_| DriveError::Store(format!("{label} exceeds postgres bigint")))
}

fn json_value<T: Serialize>(value: &T) -> tm_drive::Result<Value> {
    serde_json::to_value(value).map_err(|err| DriveError::Store(err.to_string()))
}

fn from_json<T: DeserializeOwned>(value: Value) -> tm_drive::Result<T> {
    serde_json::from_value(value).map_err(|err| DriveError::Store(err.to_string()))
}

fn enum_label<T: Serialize>(value: &T) -> tm_drive::Result<String> {
    match json_value(value)? {
        Value::String(value) => Ok(value),
        _ => Err(DriveError::Store(
            "drive enum did not serialize as a string".to_string(),
        )),
    }
}

fn map_entry_write_error(error: tokio_postgres::Error, path: &str) -> DriveError {
    if error.code() == Some(&SqlState::UNIQUE_VIOLATION) {
        DriveError::Collision(path.to_string())
    } else {
        store_error(error)
    }
}

fn store_error(error: tokio_postgres::Error) -> DriveError {
    DriveError::Store(error.to_string())
}
