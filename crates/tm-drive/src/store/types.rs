use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tm_artifacts::ArtifactStore;
use uuid::Uuid;

use crate::{
    DriveEntry, DriveEntryId, DriveOrganizerRunId, OrganizerProposal, OrganizerRun, Transduction,
};

#[derive(Debug, Clone)]
pub struct InMemoryDriveStore {
    pub(crate) artifacts: ArtifactStore,
    pub(crate) inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
pub(crate) struct Inner {
    pub(crate) entries: BTreeMap<DriveEntryId, DriveEntry>,
    pub(crate) path_to_id: BTreeMap<String, DriveEntryId>,
    pub(crate) proposals: BTreeMap<Uuid, OrganizerProposal>,
    pub(crate) organizer_runs: BTreeMap<DriveOrganizerRunId, OrganizerRun>,
    pub(crate) corrections: Vec<DriveCorrection>,
}

#[derive(Debug, Clone)]
pub(crate) struct DriveCorrection {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) created_at: DateTime<Utc>,
}

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
