use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::{HostError, Result};

use super::super::config::{FsMode, FsPolicy, ParsedPath};

pub(in crate::linked) fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))
}

pub(in crate::linked) fn validate_alias(alias: &str) -> Result<()> {
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

pub(in crate::linked) fn validate_command_name(command: &str) -> Result<()> {
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

pub(in crate::linked) fn parse_linked_path(input: &str) -> Result<ParsedPath> {
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

pub(in crate::linked) fn normalize_relative(relative: &str) -> Result<PathBuf> {
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

pub(in crate::linked) fn is_windows_drive(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/' || bytes[2] == b'\\')
}

pub(in crate::linked) fn ensure_rw(policy: &FsPolicy, display: &str) -> Result<()> {
    if policy.mode != FsMode::Rw {
        return Err(HostError::CapabilityDenied(format!(
            "{display} is read-only"
        )));
    }
    Ok(())
}

pub(in crate::linked) fn display_path(alias: &str, relative: &Path) -> String {
    let rel = path_slash(relative);
    if rel.is_empty() {
        format!("{alias}:")
    } else {
        format!("{alias}:{rel}")
    }
}

pub(in crate::linked) fn linked_uri(alias: &str, relative: &Path) -> String {
    let rel = path_slash(relative);
    if rel.is_empty() {
        format!("linked://{alias}/")
    } else {
        format!("linked://{alias}/{rel}")
    }
}

pub(in crate::linked) fn path_slash(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
