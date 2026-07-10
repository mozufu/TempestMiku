use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufRead, BufReader},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{HostError, Result};

use super::{
    config::{FsMode, FsPolicy, ParsedPath, ResolvedPath},
    tools::{FsEntry, InsertAt, PatchHunk},
};

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

pub(super) fn ensure_under_root(path: &Path, root: &Path, display: &str) -> Result<()> {
    if !path.starts_with(root) {
        return Err(HostError::InvalidPath(format!(
            "{display} resolves outside linked folder"
        )));
    }
    Ok(())
}

pub(super) fn ensure_existing_ancestor_under_root(
    parent: &Path,
    root: &Path,
    display: &str,
) -> Result<()> {
    let mut current = parent;
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| HostError::InvalidPath(format!("missing ancestor for {display}")))?;
    }
    let canonical = current
        .canonicalize()
        .map_err(|err| HostError::InvalidPath(format!("{display}: {err}")))?;
    ensure_under_root(&canonical, root, display)
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

pub(super) fn read_text_page(path: &Path, selector: Option<&str>) -> Result<(String, bool)> {
    let (start, end, byte_limit) = parse_text_selector(selector)?;
    let normalize_selected_lines = selector.is_some();
    let file = File::open(path).map_err(|err| HostError::HostCall(err.to_string()))?;
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
    if resolved.path.is_file() {
        out.push(fs_entry(
            &resolved.alias,
            &resolved.relative,
            &resolved.path,
        )?);
        return Ok(out);
    }
    visit_dir(
        &resolved.alias,
        &resolved.policy.root,
        &resolved.path,
        recursive,
        limit,
        include_hidden,
        &mut out,
    )?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

pub(super) fn visit_dir(
    alias: &str,
    root: &Path,
    dir: &Path,
    recursive: bool,
    limit: usize,
    include_hidden: bool,
    out: &mut Vec<FsEntry>,
) -> Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir)
        .map_err(|err| HostError::HostCall(err.to_string()))?
        .collect::<std::result::Result<Vec<_>, io::Error>>()
        .map_err(|err| HostError::HostCall(err.to_string()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        let name = entry.file_name();
        if !include_hidden && name.to_string_lossy().starts_with('.') {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path.as_path());
        out.push(fs_entry(alias, rel, &path)?);
        if recursive && path.is_dir() {
            visit_dir(alias, root, &path, recursive, limit, include_hidden, out)?;
        }
    }
    Ok(())
}

pub(super) fn fs_entry(alias: &str, relative: &Path, path: &Path) -> Result<FsEntry> {
    let metadata =
        fs::symlink_metadata(path).map_err(|err| HostError::HostCall(err.to_string()))?;
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "dir"
    } else {
        "file"
    };
    Ok(FsEntry {
        path: display_path(alias, relative),
        uri: linked_uri(alias, relative),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(alias)
            .to_string(),
        kind: kind.to_string(),
        size_bytes: metadata.is_file().then_some(metadata.len()),
        modified_at: metadata.modified().ok().map(system_time_rfc3339),
    })
}

pub(super) fn system_time_rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

pub(super) fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|err| HostError::InvalidArgs(err.to_string()))?);
    }
    builder
        .build()
        .map_err(|err| HostError::InvalidArgs(err.to_string()))
}

pub(super) fn load_simple_gitignore(root: &Path) -> Result<Option<GlobSet>> {
    let path = root.join(".gitignore");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|err| HostError::HostCall(err.to_string()))?;
    let patterns = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if patterns.is_empty() {
        return Ok(None);
    }
    compile_globs(&patterns).map(Some)
}

pub(super) fn collect_files(resolved: &ResolvedPath, out: &mut Vec<ResolvedPath>) -> Result<()> {
    for dent in WalkBuilder::new(&resolved.path).hidden(true).build() {
        let dent = dent.map_err(|err| HostError::HostCall(err.to_string()))?;
        let path = dent.path();
        if path == resolved.path || !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(&resolved.policy.root)
            .map_err(|err| HostError::HostCall(err.to_string()))?
            .to_path_buf();
        out.push(ResolvedPath {
            alias: resolved.alias.clone(),
            relative: relative.clone(),
            display: display_path(&resolved.alias, &relative),
            path: path.to_path_buf(),
            policy: resolved.policy.clone(),
        });
    }
    Ok(())
}

pub(super) fn file_tag(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hex::encode(hash)[..16].to_string()
}

pub(super) fn apply_line_hunks(old: &str, hunks: &[PatchHunk]) -> Result<String> {
    let had_trailing_newline = old.ends_with('\n');
    let body = if had_trailing_newline {
        &old[..old.len() - 1]
    } else {
        old
    };
    let lines: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n').map(str::to_string).collect()
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
                lines: new_lines,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: new_lines.clone(),
                });
            }
            PatchHunk::Delete {
                start_line,
                end_line,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: Vec::new(),
                });
            }
            PatchHunk::Insert {
                at,
                line,
                lines: new_lines,
            } => {
                let pos = match at {
                    InsertAt::Head => {
                        if line.is_some() {
                            return Err(HostError::InvalidArgs(
                                "insert head must not include line".to_string(),
                            ));
                        }
                        0
                    }
                    InsertAt::Tail => {
                        if line.is_some() {
                            return Err(HostError::InvalidArgs(
                                "insert tail must not include line".to_string(),
                            ));
                        }
                        lines.len()
                    }
                    InsertAt::Before => {
                        let line = line.ok_or_else(|| {
                            HostError::InvalidArgs("insert before requires line".to_string())
                        })?;
                        validate_line(line, lines.len())?;
                        line - 1
                    }
                    InsertAt::After => {
                        let line = line.ok_or_else(|| {
                            HostError::InvalidArgs("insert after requires line".to_string())
                        })?;
                        validate_line(line, lines.len())?;
                        line
                    }
                };
                inserts.entry(pos).or_default().extend(new_lines.clone());
            }
            PatchHunk::Move { .. } => {}
            PatchHunk::Remove => {}
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
    let mut new = out.join("\n");
    if had_trailing_newline {
        new.push('\n');
    }
    Ok(new)
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
    let mut diff = format!("--- {path}\n+++ {path}\n");
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

pub(super) fn stdin_present(stdin: &Option<Value>) -> bool {
    match stdin {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        Some(_) => true,
    }
}
