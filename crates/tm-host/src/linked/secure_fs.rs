use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use crate::{HostError, Result};

use super::config::FsPolicy;

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

#[cfg(not(unix))]
pub(super) fn unsupported() -> HostError {
    HostError::CapabilityDenied(
        "linked-folder access requires descriptor-relative Unix filesystem APIs".to_string(),
    )
}

#[cfg(unix)]
fn invalid_path(display: &str, error: impl std::fmt::Display) -> HostError {
    HostError::InvalidPath(format!("{display}: secure path lookup failed: {error}"))
}

#[cfg(unix)]
fn host_call(error: impl std::fmt::Display) -> HostError {
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
fn snapshot_from_stat(stat: &rustix::fs::Stat) -> EntrySnapshot {
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

pub(super) fn pin_root_identity(root: &Path, display: &str) -> Result<FileIdentity> {
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

pub(super) fn open_parent(
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

pub(super) fn stat_entry(parent: &SecureParent, display: &str) -> Result<Option<EntrySnapshot>> {
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
fn checked_open_at(
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

pub(super) fn open_existing(
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

pub(super) fn open_entry_file(
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

pub(super) fn read_bounded(mut file: File, limit: usize, display: &str) -> Result<Vec<u8>> {
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

pub(super) fn read_entry_bounded(
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

pub(super) fn create_file(parent: &SecureParent, data: &[u8], display: &str) -> Result<()> {
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

pub(super) fn atomic_replace(
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
            let mode = (permissions.mode() & 0o7777)
                .try_into()
                .expect("portable Unix permission bits fit rustix RawMode");
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

pub(super) fn rename_entry(
    source: &SecureParent,
    source_expected: EntrySnapshot,
    dest: &SecureParent,
    dest_expected: Option<EntrySnapshot>,
    display: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
        let fresh_source = stat_entry(source, display)?.ok_or_else(|| {
            HostError::InvalidArgs(format!(
                "stale filesystem entry for {display}; retry from a fresh read"
            ))
        })?;
        if fresh_source.identity != source_expected.identity
            || fresh_source.kind != source_expected.kind
        {
            return Err(HostError::InvalidArgs(format!(
                "stale filesystem entry for {display}; retry from a fresh read"
            )));
        }
        let fresh_dest = stat_entry(dest, display)?;
        if fresh_dest.map(|entry| (entry.identity, entry.kind))
            != dest_expected.map(|entry| (entry.identity, entry.kind))
        {
            return Err(HostError::InvalidArgs(format!(
                "stale destination for {display}; retry from a fresh read"
            )));
        }
        if dest_expected.is_none() {
            #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
            {
                return rustix::fs::renameat_with(
                    &source.dir,
                    source.name.as_os_str(),
                    &dest.dir,
                    dest.name.as_os_str(),
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(host_call);
            }
        }
        rustix::fs::renameat(
            &source.dir,
            source.name.as_os_str(),
            &dest.dir,
            dest.name.as_os_str(),
        )
        .map_err(host_call)
    }
    #[cfg(not(unix))]
    {
        let _ = (source, source_expected, dest, dest_expected, display);
        Err(unsupported())
    }
}

pub(super) fn remove_entry(
    parent: &SecureParent,
    expected: EntrySnapshot,
    display: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
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
        rustix::fs::unlinkat(
            &parent.dir,
            parent.name.as_os_str(),
            rustix::fs::AtFlags::empty(),
        )
        .map_err(host_call)
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, expected, display);
        Err(unsupported())
    }
}

pub(super) fn walk_secure<F>(
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
