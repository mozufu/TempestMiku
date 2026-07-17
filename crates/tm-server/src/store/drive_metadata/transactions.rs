use tm_drive::{
    DriveCorrectionRecord, DriveEntry, DriveError, DriveMoveCommit, OrganizerProposal,
    OrganizerProposalCommit, ProposalStatus, initial_record_version,
};

use super::{
    PostgresDriveMetadataStore,
    persistence::{
        insert_correction, replace_entry_children, tombstone_and_delete_entry, write_entry,
        write_proposal,
    },
    records::{
        map_entry_write_error, next_version, query_record_opt, require_version, store_error,
        version_error,
    },
};

impl PostgresDriveMetadataStore {
    pub(super) async fn commit_move_transaction(
        &self,
        commit: DriveMoveCommit,
    ) -> tm_drive::Result<DriveEntry> {
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

    pub(super) async fn commit_organizer_proposal_transaction(
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
