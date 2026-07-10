use std::fmt::Debug;

use async_trait::async_trait;
use uuid::Uuid;

use super::types::{InMemoryDriveMetadataStore, Inner};
use crate::{
    DRIVE_METADATA_SCHEMA_VERSION, DriveCorrectionRecord, DriveEntry, DriveEntryId, DriveError,
    DriveLinkRecord, DriveMetadataSnapshot, DriveOrganizerRunId, OrganizerProposal, OrganizerRun,
    ProposalStatus, initial_record_version,
};

#[derive(Debug, Clone)]
pub struct DriveEntryUpdate {
    pub expected_version: u64,
    pub replacement: DriveEntry,
}

#[derive(Debug, Clone, Copy)]
pub struct DriveOverwriteTarget {
    pub id: DriveEntryId,
    pub expected_version: u64,
}

#[derive(Debug, Clone)]
pub struct DriveMoveCommit {
    pub source: DriveEntryUpdate,
    pub overwrite: Option<DriveOverwriteTarget>,
    pub correction: DriveCorrectionRecord,
}

#[derive(Debug, Clone)]
pub struct OrganizerProposalCommit {
    pub expected_proposal_version: u64,
    pub replacement: OrganizerProposal,
    pub entry_update: Option<DriveEntryUpdate>,
    pub correction: Option<DriveCorrectionRecord>,
}

#[async_trait]
pub trait DriveMetadataStore: Debug + Send + Sync {
    async fn entry(&self, id: DriveEntryId) -> crate::Result<Option<DriveEntry>>;
    async fn entry_by_path(&self, path: &str) -> crate::Result<Option<DriveEntry>>;
    async fn entries(&self) -> crate::Result<Vec<DriveEntry>>;
    async fn insert_entry(&self, entry: DriveEntry) -> crate::Result<DriveEntry>;
    async fn compare_and_swap_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
        replacement: DriveEntry,
    ) -> crate::Result<DriveEntry>;
    async fn remove_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
    ) -> crate::Result<DriveEntry>;
    async fn commit_move(&self, commit: DriveMoveCommit) -> crate::Result<DriveEntry>;

    async fn proposals(&self) -> crate::Result<Vec<OrganizerProposal>>;
    async fn insert_proposal(
        &self,
        proposal: OrganizerProposal,
    ) -> crate::Result<OrganizerProposal>;
    async fn compare_and_swap_proposal(
        &self,
        id: Uuid,
        expected_version: u64,
        replacement: OrganizerProposal,
    ) -> crate::Result<OrganizerProposal>;
    async fn commit_organizer_proposal(
        &self,
        commit: OrganizerProposalCommit,
    ) -> crate::Result<OrganizerProposal>;

    async fn organizer_runs(&self) -> crate::Result<Vec<OrganizerRun>>;
    async fn insert_organizer_run(&self, run: OrganizerRun) -> crate::Result<OrganizerRun>;
    async fn compare_and_swap_organizer_run(
        &self,
        id: DriveOrganizerRunId,
        expected_version: u64,
        replacement: OrganizerRun,
    ) -> crate::Result<OrganizerRun>;

    async fn links(&self) -> crate::Result<Vec<DriveLinkRecord>>;
    async fn link(&self, alias: &str) -> crate::Result<Option<DriveLinkRecord>>;
    async fn insert_link(&self, link: DriveLinkRecord) -> crate::Result<DriveLinkRecord>;
    async fn compare_and_swap_link(
        &self,
        alias: &str,
        expected_version: u64,
        replacement: DriveLinkRecord,
    ) -> crate::Result<DriveLinkRecord>;

    async fn append_correction(
        &self,
        correction: DriveCorrectionRecord,
    ) -> crate::Result<DriveCorrectionRecord>;
    async fn snapshot(&self) -> crate::Result<DriveMetadataSnapshot>;
}

impl InMemoryDriveMetadataStore {
    pub fn from_snapshot(snapshot: DriveMetadataSnapshot) -> crate::Result<Self> {
        if snapshot.schema_version != DRIVE_METADATA_SCHEMA_VERSION {
            return Err(DriveError::Store(format!(
                "unsupported drive metadata schema version {}; expected {}",
                snapshot.schema_version, DRIVE_METADATA_SCHEMA_VERSION
            )));
        }

        let mut inner = Inner::default();
        for entry in snapshot.entries {
            validate_version("entry", entry.id, entry.version)?;
            if inner.entries.contains_key(&entry.id) {
                return Err(DriveError::Store(format!(
                    "duplicate drive entry id {} in snapshot",
                    entry.id
                )));
            }
            if inner.path_to_id.contains_key(&entry.path) {
                return Err(DriveError::Collision(entry.path));
            }
            inner.path_to_id.insert(entry.path.clone(), entry.id);
            inner.entries.insert(entry.id, entry);
        }
        for proposal in snapshot.proposals {
            validate_version("proposal", proposal.id, proposal.version)?;
            if inner.proposals.insert(proposal.id, proposal).is_some() {
                return Err(DriveError::Store(
                    "duplicate organizer proposal id in snapshot".to_string(),
                ));
            }
        }
        for run in snapshot.organizer_runs {
            validate_version("organizer run", run.id, run.version)?;
            if inner.organizer_runs.insert(run.id, run).is_some() {
                return Err(DriveError::Store(
                    "duplicate organizer run id in snapshot".to_string(),
                ));
            }
        }
        for link in snapshot.links {
            validate_version("link", &link.alias, link.version)?;
            if inner.links.insert(link.alias.clone(), link).is_some() {
                return Err(DriveError::Store(
                    "duplicate drive link alias in snapshot".to_string(),
                ));
            }
        }
        for correction in &snapshot.corrections {
            validate_version("correction", correction.id, correction.version)?;
        }
        inner.corrections = snapshot.corrections;

        Ok(Self {
            inner: std::sync::Arc::new(parking_lot::Mutex::new(inner)),
        })
    }

    fn snapshot_now(&self) -> DriveMetadataSnapshot {
        let inner = self.inner.lock();
        DriveMetadataSnapshot {
            schema_version: DRIVE_METADATA_SCHEMA_VERSION,
            entries: inner.entries.values().cloned().collect(),
            proposals: inner.proposals.values().cloned().collect(),
            organizer_runs: inner.organizer_runs.values().cloned().collect(),
            links: inner.links.values().cloned().collect(),
            corrections: inner.corrections.clone(),
        }
    }
}

#[async_trait]
impl DriveMetadataStore for InMemoryDriveMetadataStore {
    async fn entry(&self, id: DriveEntryId) -> crate::Result<Option<DriveEntry>> {
        Ok(self.inner.lock().entries.get(&id).cloned())
    }

    async fn entry_by_path(&self, path: &str) -> crate::Result<Option<DriveEntry>> {
        let inner = self.inner.lock();
        Ok(inner
            .path_to_id
            .get(path)
            .and_then(|id| inner.entries.get(id))
            .cloned())
    }

    async fn entries(&self) -> crate::Result<Vec<DriveEntry>> {
        Ok(self.inner.lock().entries.values().cloned().collect())
    }

    async fn insert_entry(&self, mut entry: DriveEntry) -> crate::Result<DriveEntry> {
        entry.version = initial_record_version();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.entries.get(&entry.id) {
            return Err(version_conflict("entry", entry.id, 0, existing.version));
        }
        if inner.path_to_id.contains_key(&entry.path) {
            return Err(DriveError::Collision(entry.path));
        }
        inner.path_to_id.insert(entry.path.clone(), entry.id);
        inner.entries.insert(entry.id, entry.clone());
        Ok(entry)
    }

    async fn compare_and_swap_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
        mut replacement: DriveEntry,
    ) -> crate::Result<DriveEntry> {
        let mut inner = self.inner.lock();
        let current = inner
            .entries
            .get(&id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {id}")))?;
        require_version("entry", id, expected_version, current.version)?;
        if replacement.id != id {
            return Err(DriveError::InvalidArgs(format!(
                "replacement entry id {} does not match {id}",
                replacement.id
            )));
        }
        if inner
            .path_to_id
            .get(&replacement.path)
            .is_some_and(|other| *other != id)
        {
            return Err(DriveError::Collision(replacement.path));
        }
        replacement.version = next_version("entry", id, expected_version)?;
        if current.path != replacement.path {
            inner.path_to_id.remove(&current.path);
            inner.path_to_id.insert(replacement.path.clone(), id);
        }
        inner.entries.insert(id, replacement.clone());
        Ok(replacement)
    }

    async fn remove_entry(
        &self,
        id: DriveEntryId,
        expected_version: u64,
    ) -> crate::Result<DriveEntry> {
        let mut inner = self.inner.lock();
        let current = inner
            .entries
            .get(&id)
            .cloned()
            .ok_or_else(|| DriveError::NotFound(format!("drive entry {id}")))?;
        require_version("entry", id, expected_version, current.version)?;
        inner.entries.remove(&id);
        inner.path_to_id.remove(&current.path);
        Ok(current)
    }

    async fn commit_move(&self, commit: DriveMoveCommit) -> crate::Result<DriveEntry> {
        let mut inner = self.inner.lock();
        let source_id = commit.source.replacement.id;
        let current = inner
            .entries
            .get(&source_id)
            .cloned()
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
            let overwritten = inner
                .entries
                .get(&target.id)
                .cloned()
                .ok_or_else(|| DriveError::NotFound(format!("drive entry {}", target.id)))?;
            require_version(
                "entry",
                target.id,
                target.expected_version,
                overwritten.version,
            )?;
            if overwritten.path != commit.source.replacement.path {
                return Err(DriveError::InvalidArgs(
                    "overwrite target no longer occupies the move destination".to_string(),
                ));
            }
            Some(overwritten)
        } else {
            None
        };
        if let Some(occupant) = inner.path_to_id.get(&commit.source.replacement.path)
            && *occupant != source_id
            && overwritten
                .as_ref()
                .is_none_or(|entry| entry.id != *occupant)
        {
            return Err(DriveError::Collision(
                commit.source.replacement.path.clone(),
            ));
        }
        reject_duplicate_correction(&inner, commit.correction.id)?;

        let mut replacement = commit.source.replacement;
        replacement.version = next_version("entry", source_id, commit.source.expected_version)?;
        let mut correction = commit.correction;
        correction.version = initial_record_version();

        if let Some(overwritten) = overwritten {
            inner.entries.remove(&overwritten.id);
            inner.path_to_id.remove(&overwritten.path);
        }
        if current.path != replacement.path {
            inner.path_to_id.remove(&current.path);
            inner.path_to_id.insert(replacement.path.clone(), source_id);
        }
        inner.entries.insert(source_id, replacement.clone());
        inner.corrections.push(correction);
        Ok(replacement)
    }

    async fn proposals(&self) -> crate::Result<Vec<OrganizerProposal>> {
        Ok(self.inner.lock().proposals.values().cloned().collect())
    }

    async fn insert_proposal(
        &self,
        mut proposal: OrganizerProposal,
    ) -> crate::Result<OrganizerProposal> {
        proposal.version = initial_record_version();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.proposals.get(&proposal.id) {
            return Err(version_conflict(
                "proposal",
                proposal.id,
                0,
                existing.version,
            ));
        }
        inner.proposals.insert(proposal.id, proposal.clone());
        Ok(proposal)
    }

    async fn compare_and_swap_proposal(
        &self,
        id: Uuid,
        expected_version: u64,
        mut replacement: OrganizerProposal,
    ) -> crate::Result<OrganizerProposal> {
        let mut inner = self.inner.lock();
        let current = inner
            .proposals
            .get(&id)
            .ok_or_else(|| DriveError::NotFound(format!("organizer proposal {id}")))?;
        require_version("proposal", id, expected_version, current.version)?;
        if replacement.id != id {
            return Err(DriveError::InvalidArgs(format!(
                "replacement proposal id {} does not match {id}",
                replacement.id
            )));
        }
        replacement.version = next_version("proposal", id, expected_version)?;
        inner.proposals.insert(id, replacement.clone());
        Ok(replacement)
    }

    async fn commit_organizer_proposal(
        &self,
        commit: OrganizerProposalCommit,
    ) -> crate::Result<OrganizerProposal> {
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

        let mut inner = self.inner.lock();
        let proposal_id = commit.replacement.id;
        let current_proposal = inner
            .proposals
            .get(&proposal_id)
            .cloned()
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

        let entry_change = if let Some(update) = commit.entry_update {
            let entry_id = update.replacement.id;
            if entry_id != current_proposal.entry_id {
                return Err(DriveError::InvalidArgs(format!(
                    "organizer entry {entry_id} does not match proposal entry {}",
                    current_proposal.entry_id
                )));
            }
            let current_entry = inner
                .entries
                .get(&entry_id)
                .cloned()
                .ok_or_else(|| DriveError::NotFound(format!("drive entry {entry_id}")))?;
            require_version(
                "entry",
                entry_id,
                update.expected_version,
                current_entry.version,
            )?;
            if let Some(occupant) = inner.path_to_id.get(&update.replacement.path)
                && *occupant != entry_id
            {
                return Err(DriveError::Collision(update.replacement.path));
            }
            if let Some(correction) = commit.correction.as_ref() {
                validate_move_correction(&current_entry, &update.replacement, correction)?;
                reject_duplicate_correction(&inner, correction.id)?;
            }
            let mut replacement = update.replacement;
            replacement.version = next_version("entry", entry_id, update.expected_version)?;
            Some((current_entry, replacement))
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

        if let Some((current_entry, replacement_entry)) = entry_change {
            if current_entry.path != replacement_entry.path {
                inner.path_to_id.remove(&current_entry.path);
                inner
                    .path_to_id
                    .insert(replacement_entry.path.clone(), replacement_entry.id);
            }
            inner
                .entries
                .insert(replacement_entry.id, replacement_entry);
        }
        if let Some(correction) = correction {
            inner.corrections.push(correction);
        }
        inner
            .proposals
            .insert(proposal_id, replacement_proposal.clone());
        Ok(replacement_proposal)
    }

    async fn organizer_runs(&self) -> crate::Result<Vec<OrganizerRun>> {
        Ok(self.inner.lock().organizer_runs.values().cloned().collect())
    }

    async fn insert_organizer_run(&self, mut run: OrganizerRun) -> crate::Result<OrganizerRun> {
        run.version = initial_record_version();
        let mut inner = self.inner.lock();
        if matches!(
            run.status,
            crate::OrganizerRunStatus::Queued | crate::OrganizerRunStatus::Running
        ) && let Some(existing) = inner.organizer_runs.values().find(|existing| {
            matches!(
                existing.status,
                crate::OrganizerRunStatus::Queued | crate::OrganizerRunStatus::Running
            )
        }) {
            return Err(version_conflict(
                "organizer run",
                "active",
                0,
                existing.version,
            ));
        }
        if let Some(existing) = inner.organizer_runs.get(&run.id) {
            return Err(version_conflict(
                "organizer run",
                run.id,
                0,
                existing.version,
            ));
        }
        inner.organizer_runs.insert(run.id, run.clone());
        Ok(run)
    }

    async fn compare_and_swap_organizer_run(
        &self,
        id: DriveOrganizerRunId,
        expected_version: u64,
        mut replacement: OrganizerRun,
    ) -> crate::Result<OrganizerRun> {
        let mut inner = self.inner.lock();
        let current = inner
            .organizer_runs
            .get(&id)
            .ok_or_else(|| DriveError::NotFound(format!("organizer run {id}")))?;
        require_version("organizer run", id, expected_version, current.version)?;
        if replacement.id != id {
            return Err(DriveError::InvalidArgs(format!(
                "replacement organizer run id {} does not match {id}",
                replacement.id
            )));
        }
        replacement.version = next_version("organizer run", id, expected_version)?;
        inner.organizer_runs.insert(id, replacement.clone());
        Ok(replacement)
    }

    async fn links(&self) -> crate::Result<Vec<DriveLinkRecord>> {
        Ok(self.inner.lock().links.values().cloned().collect())
    }

    async fn link(&self, alias: &str) -> crate::Result<Option<DriveLinkRecord>> {
        Ok(self.inner.lock().links.get(alias).cloned())
    }

    async fn insert_link(&self, mut link: DriveLinkRecord) -> crate::Result<DriveLinkRecord> {
        link.version = initial_record_version();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.links.get(&link.alias) {
            return Err(version_conflict("link", &link.alias, 0, existing.version));
        }
        inner.links.insert(link.alias.clone(), link.clone());
        Ok(link)
    }

    async fn compare_and_swap_link(
        &self,
        alias: &str,
        expected_version: u64,
        mut replacement: DriveLinkRecord,
    ) -> crate::Result<DriveLinkRecord> {
        let mut inner = self.inner.lock();
        let current = inner
            .links
            .get(alias)
            .ok_or_else(|| DriveError::NotFound(format!("drive link {alias}")))?;
        require_version("link", alias, expected_version, current.version)?;
        if replacement.alias != alias {
            return Err(DriveError::InvalidArgs(format!(
                "replacement link alias {} does not match {alias}",
                replacement.alias
            )));
        }
        replacement.version = next_version("link", alias, expected_version)?;
        inner.links.insert(alias.to_string(), replacement.clone());
        Ok(replacement)
    }

    async fn append_correction(
        &self,
        mut correction: DriveCorrectionRecord,
    ) -> crate::Result<DriveCorrectionRecord> {
        correction.version = initial_record_version();
        self.inner.lock().corrections.push(correction.clone());
        Ok(correction)
    }

    async fn snapshot(&self) -> crate::Result<DriveMetadataSnapshot> {
        Ok(self.snapshot_now())
    }
}

fn require_version(
    entity: &'static str,
    id: impl ToString,
    expected: u64,
    actual: u64,
) -> crate::Result<()> {
    if expected == actual {
        return Ok(());
    }
    Err(version_conflict(entity, id, expected, actual))
}

fn validate_move_correction(
    current: &DriveEntry,
    replacement: &DriveEntry,
    correction: &DriveCorrectionRecord,
) -> crate::Result<()> {
    if correction.from != current.path || correction.to != replacement.path {
        return Err(DriveError::InvalidArgs(
            "drive correction does not match move source and destination".to_string(),
        ));
    }
    Ok(())
}

fn reject_duplicate_correction(inner: &Inner, id: Uuid) -> crate::Result<()> {
    if inner
        .corrections
        .iter()
        .any(|correction| correction.id == id)
    {
        return Err(version_conflict("correction", id, 0, 1));
    }
    Ok(())
}

fn version_conflict(
    entity: &'static str,
    id: impl ToString,
    expected: u64,
    actual: u64,
) -> DriveError {
    DriveError::Conflict {
        entity,
        id: id.to_string(),
        expected,
        actual,
    }
}

fn next_version(entity: &'static str, id: impl ToString, current: u64) -> crate::Result<u64> {
    current.checked_add(1).ok_or_else(|| {
        DriveError::Store(format!(
            "drive {entity} {} version overflow",
            id.to_string()
        ))
    })
}

fn validate_version(entity: &'static str, id: impl ToString, version: u64) -> crate::Result<()> {
    if version >= initial_record_version() {
        return Ok(());
    }
    Err(DriveError::Store(format!(
        "drive {entity} {} has invalid version {version}",
        id.to_string()
    )))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tm_artifacts::ArtifactStore;
    use tm_host::FsMode;

    use super::*;
    use crate::{
        DriveOperations, DriveOrganizerConfig, DrivePutOptions, DriveService, ProposalStatus,
        drive_link_plan,
    };

    #[tokio::test]
    async fn snapshot_restart_preserves_mutable_metadata_and_revoked_links() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::open(root.path(), "drive").unwrap();
        let drive = DriveService::new(artifacts.clone());
        let filed = DriveOperations::put_bytes(
            &drive,
            b"# Durable note\nmetadata survives restart",
            DrivePutOptions {
                auto: true,
                suggested_path: Some("inbox/durable.md".to_string()),
                tags: vec!["initial".to_string()],
                ..DrivePutOptions::default()
            },
        )
        .await
        .unwrap();
        DriveOperations::tag_entry(&drive, &filed.uri, vec!["review".to_string()])
            .await
            .unwrap();
        let linked = tempfile::tempdir().unwrap();
        let plan = drive_link_plan(linked.path(), FsMode::Ro, Some("durable-project")).unwrap();
        DriveOperations::record_link(&drive, &plan).await.unwrap();
        let invalidated = DriveOperations::invalidate_link(
            &drive,
            &plan.alias,
            "upstream returned sk-testsecret123456",
        )
        .await
        .unwrap();
        assert!(
            !invalidated.metadata["invalidReason"]
                .as_str()
                .unwrap()
                .contains("sk-testsecret123456")
        );
        DriveOperations::revoke_link(&drive, &plan.alias)
            .await
            .unwrap();

        let snapshot = drive.metadata_snapshot().await.unwrap();
        let restarted = DriveService::from_snapshot(artifacts, snapshot.clone()).unwrap();
        assert_eq!(restarted.metadata_snapshot().await.unwrap(), snapshot);
        let proposals = DriveOperations::proposals(&restarted).await.unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].status, ProposalStatus::Pending);
        let links = DriveOperations::links(&restarted).await.unwrap();
        assert_eq!(links[0].status, crate::DriveLinkStatus::Revoked);
        assert!(links[0].version > initial_record_version());
    }

    #[tokio::test]
    async fn compare_and_swap_rejects_a_concurrent_stale_entry_update() {
        let root = tempfile::tempdir().unwrap();
        let drive = DriveService::new(ArtifactStore::open(root.path(), "drive").unwrap());
        let filed = DriveOperations::put_bytes(
            &drive,
            b"concurrency",
            DrivePutOptions {
                suggested_path: Some("notes/concurrency.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .await
        .unwrap();
        let metadata = drive.metadata_store();
        let mut first = filed.entry.clone();
        first.tags.push("first".to_string());
        let mut second = filed.entry.clone();
        second.tags.push("second".to_string());

        let (left, right) = tokio::join!(
            metadata.compare_and_swap_entry(filed.entry.id, filed.entry.version, first),
            metadata.compare_and_swap_entry(filed.entry.id, filed.entry.version, second),
        );
        assert_ne!(left.is_ok(), right.is_ok());
        let error = left.err().or_else(|| right.err()).unwrap();
        assert!(matches!(error, DriveError::Conflict { .. }));
        assert_eq!(
            metadata
                .entry(filed.entry.id)
                .await
                .unwrap()
                .unwrap()
                .version,
            filed.entry.version + 1
        );
    }

    #[tokio::test]
    async fn stale_overwrite_move_preserves_the_target_and_records_no_correction() {
        let root = tempfile::tempdir().unwrap();
        let drive = DriveService::new(ArtifactStore::open(root.path(), "drive").unwrap());
        let source = DriveOperations::put_bytes(
            &drive,
            b"source",
            DrivePutOptions {
                suggested_path: Some("notes/source.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .await
        .unwrap()
        .entry;
        let target = DriveOperations::put_bytes(
            &drive,
            b"target",
            DrivePutOptions {
                suggested_path: Some("notes/target.txt".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .await
        .unwrap()
        .entry;
        let metadata = drive.metadata_store();
        let mut concurrent = source.clone();
        concurrent.tags.push("concurrent".to_string());
        metadata
            .compare_and_swap_entry(source.id, source.version, concurrent)
            .await
            .unwrap();

        let mut replacement = source.clone();
        replacement.path = target.path.clone();
        replacement.uri = DriveEntry::drive_uri(&target.path);
        let error = metadata
            .commit_move(DriveMoveCommit {
                source: DriveEntryUpdate {
                    expected_version: source.version,
                    replacement,
                },
                overwrite: Some(DriveOverwriteTarget {
                    id: target.id,
                    expected_version: target.version,
                }),
                correction: DriveCorrectionRecord {
                    id: Uuid::new_v4(),
                    version: initial_record_version(),
                    from: source.path.clone(),
                    to: target.path.clone(),
                    created_at: Utc::now(),
                },
            })
            .await
            .unwrap_err();
        assert!(matches!(error, DriveError::Conflict { .. }));
        assert_eq!(
            metadata
                .entry_by_path(&target.path)
                .await
                .unwrap()
                .unwrap()
                .id,
            target.id
        );
        assert_eq!(
            metadata
                .entry_by_path(&source.path)
                .await
                .unwrap()
                .unwrap()
                .id,
            source.id
        );
        assert!(metadata.snapshot().await.unwrap().corrections.is_empty());
    }

    #[tokio::test]
    async fn organizer_commit_is_atomic_under_conflict_and_retry() {
        let root = tempfile::tempdir().unwrap();
        let drive = DriveService::new(ArtifactStore::open(root.path(), "drive").unwrap());
        let entry = DriveOperations::put_bytes(
            &drive,
            b"# Raw\norganizer should move this into project notes",
            DrivePutOptions {
                suggested_path: Some("inbox/atomic.md".to_string()),
                project: Some("TempestMiku".to_string()),
                doc_kind: Some("note".to_string()),
                ..DrivePutOptions::default()
            },
        )
        .await
        .unwrap()
        .entry;
        let proposal = DriveOperations::organize_scoped_with_config(
            &drive,
            Some("TempestMiku"),
            DriveOrganizerConfig::default(),
        )
        .await
        .unwrap()
        .into_iter()
        .find(|proposal| proposal.entry_id == entry.id)
        .expect("move proposal");
        let target = proposal.proposed_path.clone().expect("proposed path");
        let now = Utc::now();
        let mut replacement_entry = entry.clone();
        replacement_entry.path = target.clone();
        replacement_entry.uri = DriveEntry::drive_uri(&target);
        replacement_entry.updated_at = now;
        let mut replacement_proposal = proposal.clone();
        replacement_proposal.status = ProposalStatus::Applied;
        replacement_proposal.updated_at = now;
        let commit = OrganizerProposalCommit {
            expected_proposal_version: proposal.version,
            replacement: replacement_proposal,
            entry_update: Some(DriveEntryUpdate {
                expected_version: entry.version,
                replacement: replacement_entry,
            }),
            correction: Some(DriveCorrectionRecord {
                id: Uuid::new_v4(),
                version: initial_record_version(),
                from: entry.path.clone(),
                to: target.clone(),
                created_at: now,
            }),
        };
        let metadata = drive.metadata_store();
        let (left, right) = tokio::join!(
            metadata.commit_organizer_proposal(commit.clone()),
            metadata.commit_organizer_proposal(commit),
        );
        assert_eq!(left.is_ok() as usize + right.is_ok() as usize, 1);
        assert!(matches!(
            left.err().or_else(|| right.err()).unwrap(),
            DriveError::Conflict { .. }
        ));

        let snapshot = metadata.snapshot().await.unwrap();
        assert_eq!(snapshot.corrections.len(), 1);
        assert_eq!(
            metadata.entry_by_path(&target).await.unwrap().unwrap().id,
            entry.id
        );
        assert_eq!(
            metadata.entry(entry.id).await.unwrap().unwrap().version,
            entry.version + 1
        );
        assert_eq!(
            metadata
                .proposals()
                .await
                .unwrap()
                .into_iter()
                .find(|candidate| candidate.id == proposal.id)
                .unwrap()
                .status,
            ProposalStatus::Applied
        );

        let retried = DriveOperations::apply_organizer_proposals(&drive, &[proposal.id])
            .await
            .unwrap();
        assert_eq!(retried[0].status, ProposalStatus::Applied);
        assert_eq!(metadata.snapshot().await.unwrap().corrections.len(), 1);
    }
}
