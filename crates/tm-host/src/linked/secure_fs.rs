use std::{fs::File, path::PathBuf, time::SystemTime};

mod mutations;
mod open;
mod read_write;
mod walk;

pub(super) use mutations::{remove_entry, rename_entry};
pub(super) use open::{open_entry_file, open_existing, open_parent, pin_root_identity, stat_entry};
pub(super) use read_write::{atomic_replace, create_file, read_bounded, read_entry_bounded};
pub(super) use walk::walk_secure;

/// Maximum descriptor/stack depth retained by one linked-folder walk.
///
/// Descriptor-relative traversal is intentionally independent of `PATH_MAX`, so an explicit bound
/// is required to keep adversarial directory trees from exhausting the host stack or file table.
pub(super) const MAX_SECURE_WALK_DEPTH: usize = 128;

/// Identity captured from the directory entry rather than from a later path lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FileIdentity {
    pub(super) device: u64,
    pub(super) inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SecureKind {
    File,
    Directory,
    Symlink,
    Other,
}

pub(super) struct SecureHandle {
    pub(super) file: File,
    pub(super) identity: FileIdentity,
    pub(super) kind: SecureKind,
    pub(super) size_bytes: Option<u64>,
    pub(super) modified_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EntrySnapshot {
    pub(super) identity: FileIdentity,
    pub(super) kind: SecureKind,
    pub(super) size_bytes: Option<u64>,
}

pub(super) struct SecureParent {
    pub(super) dir: File,
    pub(super) name: std::ffi::OsString,
}

pub(super) struct SecureWalkEntry {
    pub(super) relative: PathBuf,
    pub(super) kind: SecureKind,
    pub(super) size_bytes: Option<u64>,
    pub(super) modified_at: Option<SystemTime>,
    pub(super) identity: FileIdentity,
    /// Present for regular files. The descriptor was opened with `O_NOFOLLOW` and its identity
    /// was checked against the directory entry before this callback is invoked.
    pub(super) file: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WalkControl {
    Continue,
    SkipDirectory,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WalkOptions {
    pub(super) recursive: bool,
    pub(super) include_hidden: bool,
    pub(super) max_visited: usize,
}
