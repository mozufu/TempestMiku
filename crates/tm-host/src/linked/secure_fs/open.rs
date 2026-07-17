use std::{
    fs::File,
    path::{Component, Path},
};

use crate::{HostError, Result};

use super::super::config::FsPolicy;
use super::{EntrySnapshot, FileIdentity, SecureHandle, SecureKind, SecureParent};

#[cfg(not(unix))]
pub(super) fn unsupported() -> HostError {
    HostError::CapabilityDenied(
        "linked-folder access requires descriptor-relative Unix filesystem APIs".to_string(),
    )
}

#[cfg(unix)]
pub(super) fn invalid_path(display: &str, error: impl std::fmt::Display) -> HostError {
    HostError::InvalidPath(format!("{display}: secure path lookup failed: {error}"))
}

#[cfg(unix)]
pub(super) fn host_call(error: impl std::fmt::Display) -> HostError {
    HostError::HostCall(error.to_string())
}

#[cfg(unix)]
fn identity_from_metadata(metadata: &std::fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(unix)]
fn identity_from_stat(stat: &rustix::fs::Stat) -> FileIdentity {
    fn raw_to_u64<T: TryInto<u64>>(value: T) -> u64 {
        value.try_into().unwrap_or_default()
    }
    FileIdentity {
        device: raw_to_u64(stat.st_dev),
        inode: raw_to_u64(stat.st_ino),
    }
}

#[cfg(unix)]
fn kind_from_stat(stat: &rustix::fs::Stat) -> SecureKind {
    match rustix::fs::FileType::from_raw_mode(stat.st_mode as _) {
        rustix::fs::FileType::RegularFile => SecureKind::File,
        rustix::fs::FileType::Directory => SecureKind::Directory,
        rustix::fs::FileType::Symlink => SecureKind::Symlink,
        _ => SecureKind::Other,
    }
}

#[cfg(unix)]
pub(super) fn snapshot_from_stat(stat: &rustix::fs::Stat) -> EntrySnapshot {
    let kind = kind_from_stat(stat);
    EntrySnapshot {
        identity: identity_from_stat(stat),
        kind,
        size_bytes: (kind == SecureKind::File)
            .then(|| u64::try_from(stat.st_size).ok())
            .flatten(),
    }
}

#[cfg(unix)]
fn directory_flags() -> rustix::fs::OFlags {
    rustix::fs::OFlags::RDONLY
        | rustix::fs::OFlags::DIRECTORY
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::CLOEXEC
}

#[cfg(unix)]
fn file_flags() -> rustix::fs::OFlags {
    rustix::fs::OFlags::RDONLY
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::NONBLOCK
        | rustix::fs::OFlags::CLOEXEC
}

#[cfg(unix)]
fn open_root_path(root: &Path, display: &str) -> Result<File> {
    use std::os::unix::ffi::OsStrExt;

    let mut current: File = rustix::fs::openat(
        rustix::fs::CWD,
        Path::new("/"),
        directory_flags(),
        rustix::fs::Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| invalid_path(display, error))?;
    for component in root.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => {
                if part.as_bytes().is_empty() {
                    continue;
                }
                current = rustix::fs::openat(
                    &current,
                    part,
                    directory_flags(),
                    rustix::fs::Mode::empty(),
                )
                .map(File::from)
                .map_err(|error| invalid_path(display, error))?;
            }
            Component::ParentDir | Component::Prefix(_) => {
                return Err(HostError::InvalidPath(format!(
                    "{display}: linked root is not a normalized absolute path"
                )));
            }
        }
    }
    Ok(current)
}

pub(in crate::linked) fn pin_root_identity(root: &Path, display: &str) -> Result<FileIdentity> {
    #[cfg(unix)]
    {
        let root = open_root_path(root, display)?;
        let metadata = root.metadata().map_err(host_call)?;
        Ok(identity_from_metadata(&metadata))
    }
    #[cfg(not(unix))]
    {
        let _ = (root, display);
        Err(unsupported())
    }
}

#[cfg(unix)]
fn open_root(policy: &FsPolicy, expected_identity: FileIdentity, display: &str) -> Result<File> {
    let root = open_root_path(&policy.root, display)?;
    let actual_identity = identity_from_metadata(&root.metadata().map_err(host_call)?);
    if actual_identity != expected_identity {
        return Err(HostError::InvalidPath(format!(
            "{display}: linked root identity changed; relink it explicitly"
        )));
    }
    Ok(root)
}

#[cfg(unix)]
fn open_relative_dir(
    policy: &FsPolicy,
    root_identity: FileIdentity,
    relative: &Path,
    create: bool,
    display: &str,
) -> Result<File> {
    let mut current = open_root(policy, root_identity, display)?;
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(HostError::InvalidPath(format!(
                "{display}: invalid relative path component"
            )));
        };
        match rustix::fs::openat(&current, part, directory_flags(), rustix::fs::Mode::empty()) {
            Ok(fd) => current = File::from(fd),
            Err(error) if create && error == rustix::io::Errno::NOENT => {
                match rustix::fs::mkdirat(&current, part, rustix::fs::Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(invalid_path(display, error)),
                }
                current = rustix::fs::openat(
                    &current,
                    part,
                    directory_flags(),
                    rustix::fs::Mode::empty(),
                )
                .map(File::from)
                .map_err(|error| invalid_path(display, error))?;
            }
            Err(error) => return Err(invalid_path(display, error)),
        }
    }
    Ok(current)
}

pub(in crate::linked) fn open_parent(
    policy: &FsPolicy,
    root_identity: FileIdentity,
    relative: &Path,
    create_parents: bool,
    display: &str,
) -> Result<SecureParent> {
    #[cfg(unix)]
    {
        let name = relative
            .file_name()
            .ok_or_else(|| HostError::InvalidPath(format!("missing file name for {display}")))?;
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        Ok(SecureParent {
            dir: open_relative_dir(policy, root_identity, parent, create_parents, display)?,
            name: name.to_os_string(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (policy, root_identity, relative, create_parents, display);
        Err(unsupported())
    }
}

pub(in crate::linked) fn stat_entry(
    parent: &SecureParent,
    display: &str,
) -> Result<Option<EntrySnapshot>> {
    #[cfg(unix)]
    {
        match rustix::fs::statat(
            &parent.dir,
            parent.name.as_os_str(),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(stat) => Ok(Some(snapshot_from_stat(&stat))),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(invalid_path(display, error)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, display);
        Err(unsupported())
    }
}

#[cfg(unix)]
pub(super) fn checked_open_at(
    parent: &File,
    name: &std::ffi::OsStr,
    expected: EntrySnapshot,
    display: &str,
) -> Result<SecureHandle> {
    let flags = if expected.kind == SecureKind::Directory {
        directory_flags()
    } else {
        file_flags()
    };
    let file = rustix::fs::openat(parent, name, flags, rustix::fs::Mode::empty())
        .map(File::from)
        .map_err(|error| invalid_path(display, error))?;
    let metadata = file.metadata().map_err(host_call)?;
    let actual = identity_from_metadata(&metadata);
    if actual != expected.identity {
        return Err(HostError::InvalidArgs(format!(
            "stale filesystem entry for {display}; retry from a fresh read"
        )));
    }
    let kind = if metadata.is_file() {
        SecureKind::File
    } else if metadata.is_dir() {
        SecureKind::Directory
    } else {
        SecureKind::Other
    };
    if kind != expected.kind {
        return Err(HostError::InvalidArgs(format!(
            "filesystem entry type changed for {display}; retry from a fresh read"
        )));
    }
    Ok(SecureHandle {
        file,
        identity: actual,
        kind,
        size_bytes: metadata.is_file().then_some(metadata.len()),
        modified_at: metadata.modified().ok(),
    })
}

pub(in crate::linked) fn open_existing(
    policy: &FsPolicy,
    root_identity: FileIdentity,
    relative: &Path,
    display: &str,
) -> Result<SecureHandle> {
    #[cfg(unix)]
    {
        if relative.as_os_str().is_empty() {
            let file = open_root(policy, root_identity, display)?;
            let metadata = file.metadata().map_err(host_call)?;
            return Ok(SecureHandle {
                identity: identity_from_metadata(&metadata),
                kind: SecureKind::Directory,
                size_bytes: None,
                modified_at: metadata.modified().ok(),
                file,
            });
        }
        let parent = open_parent(policy, root_identity, relative, false, display)?;
        let snapshot = stat_entry(&parent, display)?.ok_or_else(|| {
            HostError::InvalidPath(format!("{display}: linked path does not exist"))
        })?;
        if snapshot.kind == SecureKind::Symlink {
            return Err(HostError::InvalidPath(format!(
                "{display}: symlink traversal is not allowed"
            )));
        }
        if !matches!(snapshot.kind, SecureKind::File | SecureKind::Directory) {
            return Err(HostError::InvalidPath(format!(
                "{display}: unsupported filesystem object"
            )));
        }
        checked_open_at(&parent.dir, parent.name.as_os_str(), snapshot, display)
    }
    #[cfg(not(unix))]
    {
        let _ = (policy, root_identity, relative, display);
        Err(unsupported())
    }
}

pub(in crate::linked) fn open_entry_file(
    parent: &SecureParent,
    snapshot: EntrySnapshot,
    display: &str,
) -> Result<File> {
    #[cfg(unix)]
    {
        if snapshot.kind != SecureKind::File {
            return Err(HostError::InvalidPath(format!(
                "{display} is not a regular file"
            )));
        }
        checked_open_at(&parent.dir, parent.name.as_os_str(), snapshot, display)
            .map(|handle| handle.file)
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, snapshot, display);
        Err(unsupported())
    }
}
