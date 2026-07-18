use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use crate::{HostError, Result};

use super::env;

const INHERITED_ENVIRONMENT: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "NIX_PATH",
    "NIX_PROFILES",
    "NIX_SSL_CERT_FILE",
    "NIX_USER_PROFILE_DIR",
    "SDKROOT",
    "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET",
];

pub(super) fn inherited_environment() -> Result<Vec<(String, OsString)>> {
    let raw_path = env::var_os("PATH").unwrap_or_default();
    let path = sanitize_path(&raw_path)?;
    let mut inherited = vec![("PATH".to_string(), path)];
    inherited.extend(
        INHERITED_ENVIRONMENT
            .iter()
            .filter_map(|key| env::var_os(key).map(|value| ((*key).to_string(), value))),
    );
    Ok(inherited)
}

fn sanitize_path(raw_path: &OsStr) -> Result<OsString> {
    let mut seen = std::collections::BTreeSet::new();
    let path_entries = env::split_paths(raw_path)
        .filter(|path| path.is_absolute())
        .filter_map(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir())
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();
    if path_entries.is_empty() {
        return Err(HostError::CapabilityDenied(
            "proc.run PATH has no absolute executable search directories".to_string(),
        ));
    }
    env::join_paths(path_entries)
        .map_err(|err| HostError::HostCall(format!("failed to sanitize proc.run PATH: {err}")))
}

pub(super) fn resolve_executable(
    command: &str,
    sanitized_path: &OsStr,
) -> Result<(PathBuf, PathBuf, (u64, u64))> {
    for directory in env::split_paths(sanitized_path) {
        let candidate = directory.join(command);
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        let Ok(metadata) = canonical.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
            return Ok((candidate, canonical, (metadata.dev(), metadata.ino())));
        }
        #[cfg(not(unix))]
        {
            return Ok((candidate, canonical, (0, 0)));
        }
    }
    Err(HostError::CapabilityDenied(format!(
        "allowlisted command {command} was not found in sanitized PATH"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_path_drops_relative_and_empty_search_entries() {
        let absolute = tempfile::tempdir().unwrap();
        let raw = env::join_paths([
            PathBuf::from("."),
            PathBuf::new(),
            absolute.path().to_path_buf(),
        ])
        .unwrap();
        let sanitized = sanitize_path(&raw).unwrap();
        let entries = env::split_paths(&sanitized).collect::<Vec<_>>();
        assert_eq!(entries, vec![absolute.path().canonicalize().unwrap()]);
        assert!(entries.iter().all(|path| path.is_absolute()));
    }
}
