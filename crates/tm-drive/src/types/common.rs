use uuid::Uuid;

pub type DriveEntryId = Uuid;
pub type DriveRunId = Uuid;
pub type DriveOrganizerRunId = Uuid;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DriveError {
    #[error("drive entry not found: {0}")]
    NotFound(String),
    #[error("invalid drive args: {0}")]
    InvalidArgs(String),
    #[error("invalid drive path: {0}")]
    InvalidPath(String),
    #[error("drive collision at {0}")]
    Collision(String),
    #[error(
        "drive {entity} {id} version conflict: expected version {expected}, actual version {actual}"
    )]
    Conflict {
        entity: &'static str,
        id: String,
        expected: u64,
        actual: u64,
    },
    #[error("drive integrity check failed for {path}: expected {expected}, got {actual}")]
    Integrity {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("drive store error: {0}")]
    Store(String),
}

pub type Result<T, E = DriveError> = std::result::Result<T, E>;

pub const DRIVE_METADATA_SCHEMA_VERSION: u32 = 1;

pub const fn initial_record_version() -> u64 {
    1
}

pub const DRIVE_EVENT_NAMES: &[&str] = &[
    "drive_put",
    "drive_transduced",
    "drive_path_proposed",
    "drive_write_proposed",
    "drive_filed",
    "drive_moved",
    "drive_tagged",
    "project_linked",
    "project_unlinked",
    "drive_organizer_started",
    "drive_organizer_completed",
    "drive_organizer_failed",
];
