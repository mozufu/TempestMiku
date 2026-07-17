use serde::{Deserialize, Serialize};

use super::{
    common::DRIVE_METADATA_SCHEMA_VERSION,
    entry::DriveEntry,
    links::{DriveCorrectionRecord, DriveLinkRecord},
    organizer::{OrganizerProposal, OrganizerRun},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveMetadataSnapshot {
    #[serde(default = "default_metadata_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<DriveEntry>,
    #[serde(default)]
    pub proposals: Vec<OrganizerProposal>,
    #[serde(default)]
    pub organizer_runs: Vec<OrganizerRun>,
    #[serde(default)]
    pub links: Vec<DriveLinkRecord>,
    #[serde(default)]
    pub corrections: Vec<DriveCorrectionRecord>,
}

impl Default for DriveMetadataSnapshot {
    fn default() -> Self {
        Self {
            schema_version: DRIVE_METADATA_SCHEMA_VERSION,
            entries: Vec::new(),
            proposals: Vec::new(),
            organizer_runs: Vec::new(),
            links: Vec::new(),
            corrections: Vec::new(),
        }
    }
}

const fn default_metadata_schema_version() -> u32 {
    DRIVE_METADATA_SCHEMA_VERSION
}
