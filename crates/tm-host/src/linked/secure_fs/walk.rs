use std::{fs::File, path::Path};

use crate::{HostError, Result};

use super::super::config::FsPolicy;
use super::open::open_existing;
#[cfg(not(unix))]
use super::open::unsupported;
#[cfg(unix)]
use super::open::{checked_open_at, host_call, snapshot_from_stat};
use super::{
    FileIdentity, MAX_SECURE_WALK_DEPTH, SecureKind, SecureWalkEntry, WalkControl, WalkOptions,
};

pub(in crate::linked) fn walk_secure<F>(
    policy: &FsPolicy,
    root_identity: FileIdentity,
    start: &Path,
    options: WalkOptions,
    visited: &mut usize,
    callback: &mut F,
) -> Result<()>
where
    F: FnMut(SecureWalkEntry) -> Result<WalkControl>,
{
    #[cfg(unix)]
    {
        let display = format!("{}:{}", policy.alias, start.display());
        let handle = open_existing(policy, root_identity, start, &display)?;
        if handle.kind == SecureKind::File {
            let control = callback(SecureWalkEntry {
                relative: start.to_path_buf(),
                kind: SecureKind::File,
                size_bytes: handle.size_bytes,
                modified_at: handle.modified_at,
                identity: handle.identity,
                file: Some(handle.file),
            })?;
            let _ = control;
            return Ok(());
        }
        walk_dir(
            &handle.file,
            start,
            WalkDirOptions {
                depth: 0,
                recursive: options.recursive,
                include_hidden: options.include_hidden,
                max_visited: options.max_visited,
            },
            visited,
            callback,
        )
        .map(|_| ())
    }
    #[cfg(not(unix))]
    {
        let _ = (policy, root_identity, start, options, visited, callback);
        Err(unsupported())
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct WalkDirOptions {
    depth: usize,
    recursive: bool,
    include_hidden: bool,
    max_visited: usize,
}

#[cfg(unix)]
fn walk_dir<F>(
    dir_file: &File,
    relative: &Path,
    options: WalkDirOptions,
    visited: &mut usize,
    callback: &mut F,
) -> Result<bool>
where
    F: FnMut(SecureWalkEntry) -> Result<WalkControl>,
{
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    if options.depth > MAX_SECURE_WALK_DEPTH {
        return Err(HostError::InvalidArgs(format!(
            "linked-folder walk exceeds {MAX_SECURE_WALK_DEPTH} directory levels"
        )));
    }

    let mut dir = rustix::fs::Dir::read_from(dir_file).map_err(host_call)?;
    let mut names = Vec::new();
    for entry in &mut dir {
        let entry = entry.map_err(host_call)?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        *visited = visited.saturating_add(1);
        if *visited > options.max_visited {
            return Err(HostError::InvalidArgs(format!(
                "linked-folder walk exceeds {} entries",
                options.max_visited
            )));
        }
        let name = std::ffi::OsString::from_vec(bytes.to_vec());
        if !options.include_hidden && name.as_bytes().starts_with(b".") {
            continue;
        }
        names.push(name);
    }
    names.sort();
    for name in names {
        let stat = match rustix::fs::statat(
            dir_file,
            name.as_os_str(),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(error) => return Err(host_call(error)),
        };
        let snapshot = snapshot_from_stat(&stat);
        let child_relative = relative.join(&name);
        let child_display = child_relative.to_string_lossy();
        let (file, modified_at) = match snapshot.kind {
            SecureKind::File | SecureKind::Directory => {
                let handle = checked_open_at(dir_file, name.as_os_str(), snapshot, &child_display)?;
                (Some(handle.file), handle.modified_at)
            }
            SecureKind::Symlink | SecureKind::Other => (None, None),
        };
        let callback_file = if snapshot.kind == SecureKind::File {
            file.as_ref()
                .map(File::try_clone)
                .transpose()
                .map_err(host_call)?
        } else {
            None
        };
        let control = callback(SecureWalkEntry {
            relative: child_relative.clone(),
            kind: snapshot.kind,
            size_bytes: snapshot.size_bytes,
            modified_at,
            identity: snapshot.identity,
            file: callback_file,
        })?;
        if control == WalkControl::Stop {
            return Ok(true);
        }
        if options.recursive
            && snapshot.kind == SecureKind::Directory
            && control != WalkControl::SkipDirectory
            && walk_dir(
                file.as_ref().expect("directory descriptor"),
                &child_relative,
                WalkDirOptions {
                    depth: options.depth.saturating_add(1),
                    ..options
                },
                visited,
                callback,
            )?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
