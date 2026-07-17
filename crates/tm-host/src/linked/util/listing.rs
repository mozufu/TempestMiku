use std::{path::Path, time::SystemTime};

use chrono::{DateTime, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};

use crate::{HostError, Result};

use super::super::{
    config::{FsPolicy, ResolvedPath},
    secure_fs::{
        FileIdentity, SecureKind, SecureWalkEntry, WalkControl, WalkOptions, open_entry_file,
        open_parent, read_bounded, stat_entry, walk_secure,
    },
    tools::FsEntry,
};
use super::{
    DEFAULT_FS_RESULT_LIMIT, MAX_FS_RESULT_BYTES, MAX_FS_RESULT_LIMIT, MAX_FS_WALK_ENTRIES,
    MAX_GLOB_PATTERN_BYTES, MAX_GLOB_PATTERNS, MAX_GLOB_TOTAL_BYTES,
    path::{display_path, linked_uri},
};

const MAX_GITIGNORE_BYTES: u64 = 1024 * 1024;

pub(in crate::linked) fn list_entries(
    resolved: &ResolvedPath,
    recursive: bool,
    limit: usize,
    include_hidden: bool,
) -> Result<Vec<FsEntry>> {
    let mut out = Vec::new();
    let mut visited = 0_usize;
    let mut result_bytes = 2_usize;
    walk_secure(
        &resolved.policy,
        resolved.root_identity,
        &resolved.relative,
        WalkOptions {
            recursive,
            include_hidden,
            max_visited: MAX_FS_WALK_ENTRIES,
        },
        &mut visited,
        &mut |entry| {
            let entry = fs_entry_from_secure(&resolved.alias, entry);
            if !push_bounded_fs_entry(&mut out, entry, limit, &mut result_bytes)? {
                return Ok(WalkControl::Stop);
            }
            Ok(WalkControl::Continue)
        },
    )?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

pub(in crate::linked) fn fs_entry_from_secure(alias: &str, entry: SecureWalkEntry) -> FsEntry {
    let kind = match entry.kind {
        SecureKind::File => "file",
        SecureKind::Directory => "dir",
        SecureKind::Symlink => "symlink",
        SecureKind::Other => "other",
    };
    FsEntry {
        path: display_path(alias, &entry.relative),
        uri: linked_uri(alias, &entry.relative),
        name: entry
            .relative
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(alias)
            .to_string(),
        kind: kind.to_string(),
        size_bytes: entry.size_bytes,
        modified_at: entry.modified_at.map(system_time_rfc3339),
    }
}

pub(in crate::linked) fn push_bounded_fs_entry(
    entries: &mut Vec<FsEntry>,
    entry: FsEntry,
    limit: usize,
    result_bytes: &mut usize,
) -> Result<bool> {
    if entries.len() >= limit {
        return Ok(false);
    }
    let encoded = serde_json::to_vec(&entry).map_err(|err| HostError::HostCall(err.to_string()))?;
    let extra = encoded
        .len()
        .saturating_add(usize::from(!entries.is_empty()));
    if result_bytes.saturating_add(extra) > MAX_FS_RESULT_BYTES {
        return Ok(false);
    }
    *result_bytes += extra;
    entries.push(entry);
    Ok(true)
}

pub(in crate::linked) fn system_time_rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

pub(in crate::linked) fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    if patterns.is_empty() || patterns.len() > MAX_GLOB_PATTERNS {
        return Err(HostError::InvalidArgs(format!(
            "glob pattern count must be between 1 and {MAX_GLOB_PATTERNS}"
        )));
    }
    let mut total_bytes = 0_usize;
    for pattern in patterns {
        if pattern.len() > MAX_GLOB_PATTERN_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "glob patterns must not exceed {MAX_GLOB_PATTERN_BYTES} UTF-8 bytes"
            )));
        }
        total_bytes = total_bytes.saturating_add(pattern.len());
        if total_bytes > MAX_GLOB_TOTAL_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "glob patterns must not exceed {MAX_GLOB_TOTAL_BYTES} aggregate UTF-8 bytes"
            )));
        }
    }
    build_globs(patterns)
}

fn build_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|err| HostError::InvalidArgs(err.to_string()))?);
    }
    builder
        .build()
        .map_err(|err| HostError::InvalidArgs(err.to_string()))
}

pub(in crate::linked) fn load_simple_gitignore(
    policy: &FsPolicy,
    root_identity: FileIdentity,
) -> Result<Option<GlobSet>> {
    let relative = Path::new(".gitignore");
    let display = display_path(&policy.alias, relative);
    let parent = open_parent(policy, root_identity, relative, false, &display)?;
    let Some(snapshot) = stat_entry(&parent, &display)? else {
        return Ok(None);
    };
    if snapshot.kind == SecureKind::Symlink {
        return Err(HostError::InvalidPath(
            ".gitignore must not be a symlink".to_string(),
        ));
    }
    if snapshot.kind != SecureKind::File {
        return Err(HostError::InvalidPath(
            ".gitignore must be a regular file".to_string(),
        ));
    }
    if snapshot
        .size_bytes
        .is_some_and(|size| size > MAX_GITIGNORE_BYTES)
    {
        return Err(HostError::InvalidArgs(format!(
            ".gitignore exceeds {MAX_GITIGNORE_BYTES} bytes"
        )));
    }
    let bytes = read_bounded(
        open_entry_file(&parent, snapshot, &display)?,
        MAX_GITIGNORE_BYTES as usize,
        &display,
    )?;
    let content = String::from_utf8(bytes)
        .map_err(|_| HostError::InvalidArgs(".gitignore must contain UTF-8 text".to_string()))?;
    let mut patterns = Vec::new();
    let mut total_pattern_bytes = 0_usize;
    for pattern in content.lines().map(str::trim) {
        if pattern.is_empty() || pattern.starts_with('#') || pattern.starts_with('!') {
            continue;
        }
        if patterns.len() >= MAX_GLOB_PATTERNS {
            return Err(HostError::InvalidArgs(format!(
                ".gitignore pattern count must not exceed {MAX_GLOB_PATTERNS}"
            )));
        }
        if pattern.len() > MAX_GLOB_PATTERN_BYTES {
            return Err(HostError::InvalidArgs(format!(
                ".gitignore patterns must not exceed {MAX_GLOB_PATTERN_BYTES} UTF-8 bytes"
            )));
        }
        total_pattern_bytes = total_pattern_bytes.saturating_add(pattern.len());
        if total_pattern_bytes > MAX_GLOB_TOTAL_BYTES {
            return Err(HostError::InvalidArgs(format!(
                ".gitignore patterns must not exceed {MAX_GLOB_TOTAL_BYTES} aggregate UTF-8 bytes"
            )));
        }
        patterns.push(pattern.to_string());
    }
    if patterns.is_empty() {
        return Ok(None);
    }
    build_globs(&patterns).map(Some)
}

pub(in crate::linked) fn validate_result_limit(
    operation: &str,
    limit: Option<usize>,
) -> Result<usize> {
    let limit = limit.unwrap_or(DEFAULT_FS_RESULT_LIMIT);
    if !(1..=MAX_FS_RESULT_LIMIT).contains(&limit) {
        return Err(HostError::InvalidArgs(format!(
            "{operation} limit must be between 1 and {MAX_FS_RESULT_LIMIT}"
        )));
    }
    Ok(limit)
}

pub(in crate::linked) fn file_tag(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hex::encode(hash)[..16].to_string()
}
