use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{HostError, Result};

use super::{
    config::{FsMode, FsPolicy, ParsedPath, ResolvedPath},
    secure_fs::{
        SecureKind, SecureWalkEntry, WalkControl, WalkOptions, open_entry_file, open_parent,
        stat_entry, walk_secure,
    },
    tools::{FsEntry, PatchHunk},
};

pub(super) const DEFAULT_FS_RESULT_LIMIT: usize = 1_000;
pub(super) const MAX_FS_RESULT_LIMIT: usize = 10_000;
pub(super) const MAX_FS_WALK_ENTRIES: usize = 100_000;
pub(super) const MAX_FS_RESULT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_GLOB_PATTERNS: usize = 64;
pub(super) const MAX_GLOB_PATTERN_BYTES: usize = 4 * 1024;
pub(super) const MAX_GLOB_TOTAL_BYTES: usize = 64 * 1024;
pub(super) const MAX_SEARCH_PATHS: usize = 64;
pub(super) const MAX_SEARCH_CONTEXT_LINES: usize = 20;
pub(super) const MAX_SEARCH_PATTERN_BYTES: usize = 16 * 1024;
pub(super) const MAX_SEARCH_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub(super) const MAX_SEARCH_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub(super) const MAX_SEARCH_RESULT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_MUTATION_FILE_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_APPROVAL_ACTION_BYTES: usize = 4 * 1024;
const MAX_GITIGNORE_BYTES: u64 = 1024 * 1024;

pub(super) fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))
}

pub(super) fn validate_alias(alias: &str) -> Result<()> {
    let mut chars = alias.chars();
    let Some(first) = chars.next() else {
        return Err(HostError::InvalidArgs(
            "linked folder alias cannot be empty".to_string(),
        ));
    };
    if !first.is_ascii_alphabetic()
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(HostError::InvalidArgs(format!(
            "invalid linked folder alias {alias}"
        )));
    }
    Ok(())
}

pub(super) fn validate_command_name(command: &str) -> Result<()> {
    let forbidden = [
        ' ', '\t', '\n', '\r', ';', '&', '|', '$', '`', '>', '<', '*', '?', '(', ')',
    ];
    if command.is_empty()
        || command.contains('/')
        || command.contains('\\')
        || command.chars().any(std::path::is_separator)
        || command.chars().any(|ch| forbidden.contains(&ch))
        || !command
            .chars()
            .any(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        return Err(HostError::InvalidArgs(format!(
            "invalid command name {command}"
        )));
    }
    Ok(())
}

pub(super) fn parse_linked_path(input: &str) -> Result<ParsedPath> {
    if input.contains('\0') {
        return Err(HostError::InvalidPath("path contains NUL byte".to_string()));
    }
    if input.starts_with('/') || input.starts_with('\\') {
        return Err(HostError::InvalidPath(format!(
            "raw absolute path rejected: {input}"
        )));
    }
    if is_windows_drive(input) {
        return Err(HostError::InvalidPath(format!(
            "windows drive path rejected: {input}"
        )));
    }
    let (alias, relative) = if input.starts_with("linked://") {
        let url = Url::parse(input).map_err(|err| HostError::InvalidPath(err.to_string()))?;
        let alias = url
            .host_str()
            .ok_or_else(|| HostError::InvalidPath(format!("missing linked alias in {input}")))?;
        (
            alias.to_string(),
            url.path().trim_start_matches('/').to_string(),
        )
    } else {
        let (alias, relative) = input
            .split_once(':')
            .ok_or_else(|| HostError::InvalidPath(format!("missing linked alias in {input}")))?;
        (alias.to_string(), relative.to_string())
    };
    if alias.is_empty() {
        return Err(HostError::InvalidPath("empty linked alias".to_string()));
    }
    let relative = normalize_relative(&relative)?;
    let display = display_path(&alias, &relative);
    Ok(ParsedPath {
        alias,
        relative,
        display,
    })
}

pub(super) fn normalize_relative(relative: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    let path = Path::new(relative);
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(HostError::InvalidPath(format!(
                    "invalid relative path component in {relative}"
                )));
            }
        }
    }
    Ok(out)
}

pub(super) fn is_windows_drive(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/' || bytes[2] == b'\\')
}

pub(super) fn ensure_rw(policy: &FsPolicy, display: &str) -> Result<()> {
    if policy.mode != FsMode::Rw {
        return Err(HostError::CapabilityDenied(format!(
            "{display} is read-only"
        )));
    }
    Ok(())
}

pub(super) fn display_path(alias: &str, relative: &Path) -> String {
    let rel = path_slash(relative);
    if rel.is_empty() {
        format!("{alias}:")
    } else {
        format!("{alias}:{rel}")
    }
}

pub(super) fn linked_uri(alias: &str, relative: &Path) -> String {
    let rel = path_slash(relative);
    if rel.is_empty() {
        format!("linked://{alias}/")
    } else {
        format!("linked://{alias}/{rel}")
    }
}

pub(super) fn path_slash(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

const DEFAULT_READ_LINES: usize = 200;
const DEFAULT_READ_BYTES: usize = 64 * 1024;
const MAX_READ_LINES: usize = 1_000;
const MAX_READ_BYTES: usize = 256 * 1024;

pub(super) fn read_text_page_from_file(
    file: File,
    selector: Option<&str>,
) -> Result<(String, bool)> {
    let (start, end, byte_limit) = parse_text_selector(selector)?;
    let normalize_selected_lines = selector.is_some();
    let mut reader = BufReader::new(file);
    for _ in 1..start {
        if reader
            .skip_until(b'\n')
            .map_err(|err| HostError::HostCall(err.to_string()))?
            == 0
        {
            return Ok((String::new(), false));
        }
    }

    let mut selected = Vec::new();
    let mut truncated = false;
    for (selected_lines, _) in (start..=end).enumerate() {
        let separator = usize::from(normalize_selected_lines && selected_lines > 0);
        let remaining = byte_limit.saturating_sub(selected.len().saturating_add(separator));
        let Some((mut line, line_truncated)) = read_bounded_line(&mut reader, remaining)? else {
            break;
        };
        if normalize_selected_lines {
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if separator == 1 {
                selected.push(b'\n');
            }
        }
        selected.extend_from_slice(&line);
        if line_truncated {
            truncated = true;
            break;
        }
    }
    let has_more = truncated
        || !reader
            .fill_buf()
            .map_err(|err| HostError::HostCall(err.to_string()))?
            .is_empty();
    if let Err(error) = std::str::from_utf8(&selected) {
        if error.error_len().is_some() {
            return Err(HostError::InvalidArgs(
                "fs.read supports UTF-8 text only".to_string(),
            ));
        }
        selected.truncate(error.valid_up_to());
    }
    let selected = String::from_utf8(selected)
        .map_err(|_| HostError::InvalidArgs("fs.read supports UTF-8 text only".to_string()))?;
    Ok((selected, has_more))
}

pub(super) fn approval_action(operation: &str, details: Value) -> String {
    const MAX_FIELD_BYTES: usize = 512;
    const MAX_COLLECTION_ENTRIES: usize = 64;

    fn utf8_prefix(value: &str, limit: usize) -> &str {
        let mut end = value.len().min(limit);
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        &value[..end]
    }

    fn sanitize(value: Value) -> Value {
        match value {
            Value::String(value) => {
                let redacted = tm_memory::redact_dream_text(&value).text;
                if redacted.len() > MAX_FIELD_BYTES {
                    Value::String(format!(
                        "{}...[bounded:{} bytes]",
                        utf8_prefix(&redacted, MAX_FIELD_BYTES),
                        redacted.len()
                    ))
                } else {
                    Value::String(redacted)
                }
            }
            Value::Array(values) => {
                let original_len = values.len();
                let mut values = values
                    .into_iter()
                    .take(MAX_COLLECTION_ENTRIES)
                    .map(sanitize)
                    .collect::<Vec<_>>();
                if original_len > MAX_COLLECTION_ENTRIES {
                    values.push(serde_json::json!({
                        "boundedRemainingEntries": original_len - MAX_COLLECTION_ENTRIES
                    }));
                }
                Value::Array(values)
            }
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .take(MAX_COLLECTION_ENTRIES)
                    .map(|(key, value)| (key, sanitize(value)))
                    .collect(),
            ),
            value => value,
        }
    }

    let action = serde_json::json!({
        "operation": operation,
        "details": sanitize(details),
    });
    let encoded = serde_json::to_string(&action)
        .unwrap_or_else(|_| format!(r#"{{"operation":"{operation}","details":"unavailable"}}"#));
    if encoded.len() <= MAX_APPROVAL_ACTION_BYTES {
        return encoded;
    }
    let digest = Sha256::digest(encoded.as_bytes());
    serde_json::json!({
        "operation": operation,
        "details": {
            "bounded": true,
            "redactedSha256": hex::encode(digest),
            "encodedBytes": encoded.len(),
        }
    })
    .to_string()
}

fn parse_text_selector(selector: Option<&str>) -> Result<(usize, usize, usize)> {
    let Some(selector) = selector else {
        return Ok((1, DEFAULT_READ_LINES, DEFAULT_READ_BYTES));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let line_count = end
        .checked_sub(start)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start || line_count > MAX_READ_LINES {
        return Err(HostError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    Ok((start, end, MAX_READ_BYTES))
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some((line, false)))
            };
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let content = &buffer[..=newline];
            let available = limit.saturating_sub(line.len());
            let copied = content.len().min(available);
            line.extend_from_slice(&content[..copied]);
            if copied < content.len() {
                reader.consume(copied);
                return Ok(Some((line, true)));
            }
            reader.consume(newline + 1);
            return Ok(Some((line, false)));
        }

        let available = limit.saturating_sub(line.len());
        let buffer_len = buffer.len();
        let copied = buffer_len.min(available);
        line.extend_from_slice(&buffer[..copied]);
        reader.consume(copied);
        if copied < buffer_len || available == 0 {
            return Ok(Some((line, true)));
        }
    }
}

pub(super) fn list_entries(
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

pub(super) fn fs_entry_from_secure(alias: &str, entry: SecureWalkEntry) -> FsEntry {
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

pub(super) fn push_bounded_fs_entry(
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

pub(super) fn system_time_rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

pub(super) fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
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

pub(super) fn load_simple_gitignore(
    policy: &FsPolicy,
    root_identity: super::secure_fs::FileIdentity,
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
    let bytes = super::secure_fs::read_bounded(
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

pub(super) fn validate_result_limit(operation: &str, limit: Option<usize>) -> Result<usize> {
    let limit = limit.unwrap_or(DEFAULT_FS_RESULT_LIMIT);
    if !(1..=MAX_FS_RESULT_LIMIT).contains(&limit) {
        return Err(HostError::InvalidArgs(format!(
            "{operation} limit must be between 1 and {MAX_FS_RESULT_LIMIT}"
        )));
    }
    Ok(limit)
}

pub(super) fn file_tag(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hex::encode(hash)[..16].to_string()
}

pub(super) fn apply_line_hunks(old: &str, hunks: &[PatchHunk]) -> Result<String> {
    let newline = dominant_line_ending(old);
    let had_trailing_newline = old.ends_with(newline);
    let body = if had_trailing_newline {
        &old[..old.len() - newline.len()]
    } else {
        old
    };
    let lines: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        body.split(newline).map(str::to_string).collect()
    };
    #[derive(Clone)]
    struct Replacement {
        start: usize,
        end: usize,
        lines: Vec<String>,
    }
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut inserts: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for hunk in hunks {
        match hunk {
            PatchHunk::Replace {
                start_line,
                end_line,
                expected_lines,
                lines: new_lines,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                validate_expected_range(*start_line, *end_line, expected_lines, &lines)?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: new_lines.clone(),
                });
            }
            PatchHunk::Delete {
                start_line,
                end_line,
                expected_lines,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                validate_expected_range(*start_line, *end_line, expected_lines, &lines)?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: Vec::new(),
                });
            }
            PatchHunk::InsertBefore {
                line,
                expected_line,
                lines: new_lines,
            } => {
                validate_line(*line, lines.len())?;
                validate_expected_line(*line, expected_line, &lines)?;
                inserts
                    .entry(line - 1)
                    .or_default()
                    .extend(new_lines.clone());
            }
            PatchHunk::InsertAfter {
                line,
                expected_line,
                lines: new_lines,
            } => {
                validate_line(*line, lines.len())?;
                validate_expected_line(*line, expected_line, &lines)?;
                inserts.entry(*line).or_default().extend(new_lines.clone());
            }
            PatchHunk::Prepend { lines: new_lines } => {
                inserts.entry(0).or_default().extend(new_lines.clone());
            }
            PatchHunk::Append { lines: new_lines } => {
                inserts
                    .entry(lines.len())
                    .or_default()
                    .extend(new_lines.clone());
            }
        }
    }
    replacements.sort_by_key(|replacement| replacement.start);
    for pair in replacements.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(HostError::InvalidArgs(
                "overlapping replace/delete hunks".to_string(),
            ));
        }
    }
    for replacement in &replacements {
        if inserts
            .keys()
            .any(|position| replacement.start < *position && *position < replacement.end)
        {
            return Err(HostError::InvalidArgs(
                "insert hunk overlaps a replaced/deleted line range".to_string(),
            ));
        }
    }
    let mut out = Vec::new();
    let mut idx = 0;
    let mut replacement_idx = 0;
    while idx <= lines.len() {
        if let Some(new_lines) = inserts.get(&idx) {
            out.extend(new_lines.clone());
        }
        if idx == lines.len() {
            break;
        }
        if replacement_idx < replacements.len() && replacements[replacement_idx].start == idx {
            out.extend(replacements[replacement_idx].lines.clone());
            idx = replacements[replacement_idx].end;
            replacement_idx += 1;
            continue;
        }
        out.push(lines[idx].clone());
        idx += 1;
    }
    let mut new = out.join(newline);
    if had_trailing_newline {
        new.push_str(newline);
    }
    Ok(new)
}

fn validate_expected_range(
    start_line: usize,
    end_line: usize,
    expected_lines: &[String],
    actual_lines: &[String],
) -> Result<()> {
    let range_len = end_line - start_line + 1;
    if expected_lines.len() != range_len {
        return Err(HostError::InvalidArgs(format!(
            "fs.patch expectedLines has {} lines but range {start_line}-{end_line} has {range_len}",
            expected_lines.len()
        )));
    }
    if actual_lines[start_line - 1..end_line] != expected_lines[..] {
        return Err(HostError::InvalidArgs(format!(
            "fs.patch context mismatch at lines {start_line}-{end_line}; re-read the file and retry"
        )));
    }
    Ok(())
}

fn validate_expected_line(line: usize, expected_line: &str, actual_lines: &[String]) -> Result<()> {
    if actual_lines[line - 1] != expected_line {
        return Err(HostError::InvalidArgs(format!(
            "fs.patch context mismatch at line {line}; re-read the file and retry"
        )));
    }
    Ok(())
}

fn dominant_line_ending(text: &str) -> &'static str {
    let newline_count = text.bytes().filter(|byte| *byte == b'\n').count();
    let crlf_count = text
        .as_bytes()
        .windows(2)
        .filter(|pair| *pair == b"\r\n")
        .count();
    if newline_count > 0 && newline_count == crlf_count {
        "\r\n"
    } else {
        "\n"
    }
}

pub(super) fn validate_range(start: usize, end: usize, len: usize) -> Result<()> {
    if start == 0 || end < start || end > len {
        return Err(HostError::InvalidArgs(format!(
            "invalid line range {start}-{end} for {len} lines"
        )));
    }
    Ok(())
}

pub(super) fn validate_line(line: usize, len: usize) -> Result<()> {
    if line == 0 || line > len {
        return Err(HostError::InvalidArgs(format!(
            "invalid line {line} for {len} lines"
        )));
    }
    Ok(())
}

pub(super) fn simple_diff(old: &str, new: &str, path: &str) -> String {
    if old == new {
        return String::new();
    }
    const CONTEXT_LINES: usize = 3;
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let old_change_end = old_lines.len() - suffix;
    let new_change_end = new_lines.len() - suffix;
    let old_hunk_start = prefix.saturating_sub(CONTEXT_LINES);
    let new_hunk_start = prefix.saturating_sub(CONTEXT_LINES);
    let old_hunk_end = (old_change_end + CONTEXT_LINES).min(old_lines.len());
    let new_hunk_end = (new_change_end + CONTEXT_LINES).min(new_lines.len());

    let mut diff = format!(
        "--- {path}\n+++ {path}\n@@ -{},{} +{},{} @@\n",
        old_hunk_start + 1,
        old_hunk_end.saturating_sub(old_hunk_start),
        new_hunk_start + 1,
        new_hunk_end.saturating_sub(new_hunk_start)
    );
    for line in &old_lines[old_hunk_start..prefix] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[prefix..old_change_end] {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[prefix..new_change_end] {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[new_change_end..new_hunk_end] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    if old_lines == new_lines {
        diff.push_str("\\ No newline at end of file changed\n");
    }
    diff
}
