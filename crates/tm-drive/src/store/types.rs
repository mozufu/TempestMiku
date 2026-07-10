use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tm_artifacts::ArtifactStore;
use uuid::Uuid;

use crate::{
    DriveCorrectionRecord, DriveEntry, DriveEntryId, DriveLinkRecord, DriveOrganizerRunId,
    OrganizerProposal, OrganizerRun, Transduction,
};

#[derive(Debug)]
pub struct DriveService<M> {
    pub(crate) artifacts: ArtifactStore,
    pub(crate) metadata: Arc<M>,
}

impl<M> Clone for DriveService<M> {
    fn clone(&self) -> Self {
        Self {
            artifacts: self.artifacts.clone(),
            metadata: Arc::clone(&self.metadata),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryDriveMetadataStore {
    pub(crate) inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
pub(crate) struct Inner {
    pub(crate) entries: BTreeMap<DriveEntryId, DriveEntry>,
    pub(crate) path_to_id: BTreeMap<String, DriveEntryId>,
    pub(crate) proposals: BTreeMap<Uuid, OrganizerProposal>,
    pub(crate) organizer_runs: BTreeMap<DriveOrganizerRunId, OrganizerRun>,
    pub(crate) links: BTreeMap<String, DriveLinkRecord>,
    pub(crate) corrections: Vec<DriveCorrectionRecord>,
}

pub type InMemoryDriveStore = DriveService<InMemoryDriveMetadataStore>;

#[derive(Debug, Clone, PartialEq)]
pub struct DriveRead {
    pub entry: DriveEntry,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct DrivePutPlan {
    pub(crate) transduction: Transduction,
    pub(crate) proposed_path: String,
    pub(crate) path: String,
    pub(crate) now: DateTime<Utc>,
}
