use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, ResourceEntry};
use uuid::Uuid;

use super::types::{DrivePutPlan, DriveRead, DriveService, InMemoryDriveMetadataStore, Inner};
use crate::types::DriveError;
use crate::{
    DriveCollisionStrategy, DriveCorrectionRecord, DriveDedupeMode, DriveEntry, DriveEntryId,
    DriveEntryStatus, DriveEvidence, DriveListOptions, DriveOrganizerRunId, DriveProvenance,
    DrivePutOptions, DrivePutResult, DriveSearchOptions, DriveSearchResult, OrganizerActionKind,
    OrganizerProposal, OrganizerRun, OrganizerRunStatus, PolicyDecision, ProposalStatus,
    TransducerInput, apply_tags, drive_uri_path, generate_organizer_proposals_for_run,
    initial_record_version, parse_virtual_dir, propose_path, transduce_document,
    vdir::virtual_query_to_search,
};

mod organizer;
mod read;
mod validation;
mod write;

pub use validation::normalize_canonical_path;
use validation::normalize_optional_prefix;
pub(crate) use validation::{
    drive_error_to_host, drive_put_requires_approval, host_drive_put_options,
    linked_alias_from_target, normalize_canonical_path_path_or_uri, sanitize_drive_bytes,
    sanitize_drive_put_options, validate_drive_identifier,
};
pub(crate) use write::{provenance, unique_path, write_proposal};

fn bump_version(version: &mut u64) {
    *version = version.saturating_add(1);
}
