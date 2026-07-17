use std::fmt::Debug;

use async_trait::async_trait;
use uuid::Uuid;

use super::types::{InMemoryDriveMetadataStore, Inner};
use crate::{
    DRIVE_METADATA_SCHEMA_VERSION, DriveCorrectionRecord, DriveEntry, DriveEntryId, DriveError,
    DriveLinkRecord, DriveMetadataSnapshot, DriveOrganizerRunId, OrganizerProposal, OrganizerRun,
    ProposalStatus, initial_record_version,
};

mod in_memory;

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
mod tests;
