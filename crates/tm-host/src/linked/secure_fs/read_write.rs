use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

use crate::{HostError, Result};

use super::super::config::FsPolicy;
#[cfg(not(unix))]
use super::open::unsupported;
#[cfg(unix)]
use super::open::{host_call, invalid_path};
use super::open::{open_entry_file, open_existing, stat_entry};
use super::{EntrySnapshot, FileIdentity, SecureKind, SecureParent};

pub(in crate::linked) fn read_bounded(
    mut file: File,
    limit: usize,
    display: &str,
) -> Result<Vec<u8>> {
    let metadata = file.metadata().map_err(host_call_portable)?;
    if metadata.len() > limit as u64 {
        return Err(HostError::InvalidArgs(format!(
            "{display} exceeds the {limit}-byte host read limit"
        )));
    }
    file.seek(SeekFrom::Start(0)).map_err(host_call_portable)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(host_call_portable)?;
    if bytes.len() > limit {
        return Err(HostError::InvalidArgs(format!(
            "{display} exceeds the {limit}-byte host read limit"
        )));
    }
    Ok(bytes)
}

fn host_call_portable(error: impl std::fmt::Display) -> HostError {
    HostError::HostCall(error.to_string())
}

#[cfg(unix)]
fn normalize_symlink_target(
    policy: &FsPolicy,
    link_relative: &Path,
    target: &Path,
    display: &str,
) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    let canonical_target;
    let input = if target.is_absolute() {
        match target.strip_prefix(&policy.root) {
            Ok(relative) => relative,
            Err(_) => {
                // macOS commonly exposes `/var` while canonical roots use `/private/var`.
                // Canonicalization is used only to recover a relative name; the later open still
                // traverses from the linked-root descriptor with `O_NOFOLLOW` on every component.
                canonical_target = target.canonicalize().map_err(|_| {
                    HostError::InvalidPath(format!(
                        "{display}: symlink target cannot be resolved safely"
                    ))
                })?;
                canonical_target.strip_prefix(&policy.root).map_err(|_| {
                    HostError::InvalidPath(format!(
                        "{display}: symlink target leaves linked folder"
                    ))
                })?
            }
        }
    } else {
        if let Some(parent) = link_relative.parent() {
            for component in parent.components() {
                if let Component::Normal(part) = component {
                    out.push(part);
                }
            }
        }
        target
    };
    for component in input.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                if !out.pop() {
                    return Err(HostError::InvalidPath(format!(
                        "{display}: symlink target leaves linked folder"
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(HostError::InvalidPath(format!(
                    "{display}: invalid symlink target"
                )));
            }
        }
    }
    Ok(out)
}

pub(in crate::linked) fn read_entry_bounded(
    policy: &FsPolicy,
    root_identity: FileIdentity,
    relative: &Path,
    parent: &SecureParent,
    snapshot: EntrySnapshot,
    limit: usize,
    display: &str,
) -> Result<Vec<u8>> {
    #[cfg(unix)]
    {
        match snapshot.kind {
            SecureKind::File => {
                read_bounded(open_entry_file(parent, snapshot, display)?, limit, display)
            }
            SecureKind::Symlink => {
                use std::os::unix::ffi::OsStrExt;
                let target =
                    rustix::fs::readlinkat(&parent.dir, parent.name.as_os_str(), Vec::new())
                        .map_err(|error| invalid_path(display, error))?;
                let target = Path::new(std::ffi::OsStr::from_bytes(target.to_bytes()));
                let target_relative = normalize_symlink_target(policy, relative, target, display)?;
                let target_display = format!("{display} symlink target");
                let handle =
                    open_existing(policy, root_identity, &target_relative, &target_display)?;
                if handle.kind != SecureKind::File {
                    return Err(HostError::InvalidPath(format!(
                        "{display}: symlink target is not a regular file"
                    )));
                }
                read_bounded(handle.file, limit, display)
            }
            _ => Err(HostError::InvalidPath(format!(
                "{display} is not a regular file or file symlink"
            ))),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (
            policy,
            root_identity,
            relative,
            parent,
            snapshot,
            limit,
            display,
        );
        Err(unsupported())
    }
}

pub(in crate::linked) fn create_file(
    parent: &SecureParent,
    data: &[u8],
    display: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
        let mut file: File = rustix::fs::openat(
            &parent.dir,
            parent.name.as_os_str(),
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o666),
        )
        .map(File::from)
        .map_err(|error| invalid_path(display, error))?;
        file.write_all(data)
            .and_then(|()| file.sync_all())
            .map_err(host_call)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, data, display);
        Err(unsupported())
    }
}

pub(in crate::linked) fn atomic_replace(
    parent: &SecureParent,
    expected: EntrySnapshot,
    permissions: &std::fs::Permissions,
    data: &[u8],
    sequence: u64,
    display: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let temp_name =
            std::ffi::OsString::from(format!(".tm-host-{}-{sequence}", std::process::id()));
        let mut temp: File = rustix::fs::openat(
            &parent.dir,
            temp_name.as_os_str(),
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .map(File::from)
        .map_err(host_call)?;
        let result = (|| -> Result<()> {
            temp.write_all(data)
                .and_then(|()| temp.sync_all())
                .map_err(host_call)?;
            let mode = (permissions.mode() & 0o7777) as rustix::fs::RawMode;
            rustix::fs::fchmod(&temp, rustix::fs::Mode::from_raw_mode(mode)).map_err(host_call)?;
            let fresh = stat_entry(parent, display)?.ok_or_else(|| {
                HostError::InvalidArgs(format!(
                    "stale filesystem entry for {display}; retry from a fresh read"
                ))
            })?;
            if fresh.identity != expected.identity || fresh.kind != expected.kind {
                return Err(HostError::InvalidArgs(format!(
                    "stale filesystem entry for {display}; retry from a fresh read"
                )));
            }
            rustix::fs::renameat(
                &parent.dir,
                temp_name.as_os_str(),
                &parent.dir,
                parent.name.as_os_str(),
            )
            .map_err(host_call)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(
                &parent.dir,
                temp_name.as_os_str(),
                rustix::fs::AtFlags::empty(),
            );
        }
        result
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, expected, permissions, data, sequence, display);
        Err(unsupported())
    }
}
