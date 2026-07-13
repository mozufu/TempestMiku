use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ModeId, ReviewAddendumChange, ReviewAddendumSection, ReviewProposalTarget};

const MANAGED_MODE_ADDENDUM_VERSION: u16 = 1;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedModeAddendumInstall {
    pub mode_id: ModeId,
    pub content_digest: String,
    pub base_version: u64,
    pub base_digest: String,
    pub source_proposal_id: String,
    pub expected_active_digest: Option<String>,
    pub changes: Vec<ReviewAddendumChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedModeAddendumVersion {
    pub version: u16,
    pub mode_id: ModeId,
    pub content_digest: String,
    pub base_version: u64,
    pub base_digest: String,
    pub source_proposal_id: String,
    pub changes: Vec<ReviewAddendumChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedModeAddendumActivation {
    pub active: Option<ManagedModeAddendumVersion>,
    pub previous_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedModeAddendumState {
    pub active: Option<ManagedModeAddendumVersion>,
    pub versions: Vec<ManagedModeAddendumVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedModeAddendumError(String);

impl ManagedModeAddendumError {
    pub(crate) fn from_message(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn new(message: impl Into<String>) -> Self {
        Self::from_message(message)
    }
}

impl fmt::Display for ManagedModeAddendumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ManagedModeAddendumError {}

pub fn mode_addendum_content_digest(
    mode_id: &ModeId,
    base_version: u64,
    base_digest: &str,
    changes: &[ReviewAddendumChange],
) -> Result<String, ManagedModeAddendumError> {
    let mut content = serde_json::json!({
        "target": ReviewProposalTarget::Mode { mode_id: mode_id.clone() },
        "baseVersion": base_version,
        "baseDigest": base_digest,
        "changes": changes,
    });
    canonicalize_json(&mut content);
    let bytes = serde_json::to_vec(&content)
        .map_err(|error| ManagedModeAddendumError::new(error.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonicalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => values.iter_mut().for_each(canonicalize_json),
        serde_json::Value::Object(map) => {
            let mut entries = std::mem::take(map).into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            for (key, mut value) in entries {
                canonicalize_json(&mut value);
                map.insert(key, value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivePointer {
    active_digest: Option<String>,
}

pub(crate) fn install(
    root: &Path,
    install: ManagedModeAddendumInstall,
) -> Result<ManagedModeAddendumActivation, ManagedModeAddendumError> {
    validate_mode_id(&install.mode_id)?;
    validate_digest(&install.content_digest)?;
    if let Some(digest) = &install.expected_active_digest {
        validate_digest(digest)?;
    }
    validate_changes(&install.changes)?;

    ensure_real_dir(root)?;
    let mode_root = root.join(install.mode_id.as_str());
    let versions_root = mode_root.join("versions");
    ensure_real_dir(&mode_root)?;
    ensure_real_dir(&versions_root)?;
    let _lock = lock_mode(&mode_root)?;

    let previous_digest =
        read_active_pointer_optional(&mode_root)?.and_then(|pointer| pointer.active_digest);
    if previous_digest != install.expected_active_digest {
        return Err(ManagedModeAddendumError::new(format!(
            "managed mode addendum {} active version changed from {:?} to {:?}",
            install.mode_id, install.expected_active_digest, previous_digest
        )));
    }

    let version = ManagedModeAddendumVersion {
        version: MANAGED_MODE_ADDENDUM_VERSION,
        mode_id: install.mode_id,
        content_digest: install.content_digest,
        base_version: install.base_version,
        base_digest: install.base_digest,
        source_proposal_id: install.source_proposal_id,
        changes: install.changes,
    };
    validate_version(&version)?;
    let version_root = versions_root.join(digest_component(&version.content_digest)?);
    install_immutable_version(&versions_root, &version_root, &version)?;
    write_active_pointer(&mode_root, Some(&version.content_digest))?;
    Ok(ManagedModeAddendumActivation {
        active: Some(version),
        previous_digest,
    })
}

pub(crate) fn rollback(
    root: &Path,
    mode_id: &ModeId,
    expected_active_digest: &str,
    target_digest: Option<&str>,
) -> Result<ManagedModeAddendumActivation, ManagedModeAddendumError> {
    validate_mode_id(mode_id)?;
    validate_digest(expected_active_digest)?;
    if let Some(digest) = target_digest {
        validate_digest(digest)?;
    }
    let mode_root = root.join(mode_id.as_str());
    reject_symlink(&mode_root)?;
    let _lock = lock_mode(&mode_root)?;
    let pointer = read_active_pointer_optional(&mode_root)?.ok_or_else(|| {
        ManagedModeAddendumError::new(format!(
            "managed mode addendum {mode_id} has no active pointer"
        ))
    })?;
    if pointer.active_digest.as_deref() != Some(expected_active_digest) {
        return Err(ManagedModeAddendumError::new(format!(
            "managed mode addendum {mode_id} active version changed from {expected_active_digest} to {:?}",
            pointer.active_digest
        )));
    }
    let active = target_digest
        .map(|digest| read_version(&mode_root, digest))
        .transpose()?;
    write_active_pointer(&mode_root, target_digest)?;
    Ok(ManagedModeAddendumActivation {
        active,
        previous_digest: Some(expected_active_digest.to_string()),
    })
}

pub(crate) fn active(
    root: &Path,
    mode_id: &ModeId,
) -> Result<Option<ManagedModeAddendumVersion>, ManagedModeAddendumError> {
    validate_mode_id(mode_id)?;
    let mode_root = root.join(mode_id.as_str());
    if !mode_root.exists() {
        return Ok(None);
    }
    let Some(pointer) = read_active_pointer_optional(&mode_root)? else {
        return Ok(None);
    };
    pointer
        .active_digest
        .as_deref()
        .map(|digest| read_version(&mode_root, digest))
        .transpose()
}

pub(crate) fn state(
    root: &Path,
    mode_id: &ModeId,
) -> Result<ManagedModeAddendumState, ManagedModeAddendumError> {
    validate_mode_id(mode_id)?;
    let mode_root = root.join(mode_id.as_str());
    let active = active(root, mode_id)?;
    let versions_root = mode_root.join("versions");
    let mut versions = Vec::new();
    if versions_root.exists() {
        reject_symlink(&versions_root)?;
        for entry in
            fs::read_dir(&versions_root).map_err(|error| io_error(&versions_root, error))?
        {
            let entry = entry.map_err(|error| io_error(&versions_root, error))?;
            if entry
                .file_type()
                .map_err(|error| io_error(&entry.path(), error))?
                .is_dir()
            {
                let digest = format!("sha256:{}", entry.file_name().to_string_lossy());
                versions.push(read_version(&mode_root, &digest)?);
            }
        }
    }
    versions.sort_by(|left, right| left.content_digest.cmp(&right.content_digest));
    Ok(ManagedModeAddendumState { active, versions })
}

fn install_immutable_version(
    versions_root: &Path,
    version_root: &Path,
    version: &ManagedModeAddendumVersion,
) -> Result<(), ManagedModeAddendumError> {
    if version_root.exists() {
        let existing = read_version_from_root(version_root)?;
        if existing == *version {
            return Ok(());
        }
        return Err(ManagedModeAddendumError::new(format!(
            "managed mode addendum version {} already exists with different content",
            version.content_digest
        )));
    }
    let temp_root = versions_root.join(format!(
        ".install-{}-{}",
        std::process::id(),
        TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&temp_root).map_err(|error| io_error(&temp_root, error))?;
    let result = (|| {
        let manifest = serde_json::to_vec_pretty(version)
            .map_err(|error| ManagedModeAddendumError::new(error.to_string()))?;
        write_new_file(&temp_root.join("manifest.json"), &manifest)?;
        fs::rename(&temp_root, version_root).map_err(|error| io_error(version_root, error))?;
        sync_directory(versions_root)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp_root);
    }
    result
}

fn write_active_pointer(
    mode_root: &Path,
    active_digest: Option<&str>,
) -> Result<(), ManagedModeAddendumError> {
    let bytes = serde_json::to_vec_pretty(&ActivePointer {
        active_digest: active_digest.map(str::to_string),
    })
    .map_err(|error| ManagedModeAddendumError::new(error.to_string()))?;
    let path = mode_root.join("active.json");
    let temp = mode_root.join(format!(
        ".active-{}-{}.tmp",
        std::process::id(),
        TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    write_new_file(&temp, &bytes)?;
    let result = fs::rename(&temp, &path)
        .map_err(|error| io_error(&path, error))
        .and_then(|()| sync_directory(mode_root));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn read_active_pointer_optional(
    mode_root: &Path,
) -> Result<Option<ActivePointer>, ManagedModeAddendumError> {
    let path = mode_root.join("active.json");
    reject_symlink_if_present(&path)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(&path, error)),
    };
    let pointer: ActivePointer = serde_json::from_slice(&bytes).map_err(|error| {
        ManagedModeAddendumError::new(format!("invalid {}: {error}", path.display()))
    })?;
    if let Some(digest) = &pointer.active_digest {
        validate_digest(digest)?;
    }
    Ok(Some(pointer))
}

fn read_version(
    mode_root: &Path,
    digest: &str,
) -> Result<ManagedModeAddendumVersion, ManagedModeAddendumError> {
    let root = mode_root.join("versions").join(digest_component(digest)?);
    let version = read_version_from_root(&root)?;
    let expected_mode = mode_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ManagedModeAddendumError::new("managed mode addendum root has no valid name")
        })?;
    if version.mode_id.as_str() != expected_mode || version.content_digest != digest {
        return Err(ManagedModeAddendumError::new(format!(
            "managed mode addendum version path does not match manifest for {digest}"
        )));
    }
    Ok(version)
}

fn read_version_from_root(
    root: &Path,
) -> Result<ManagedModeAddendumVersion, ManagedModeAddendumError> {
    reject_symlink(root)?;
    let path = root.join("manifest.json");
    reject_symlink(&path)?;
    let version: ManagedModeAddendumVersion =
        serde_json::from_slice(&fs::read(&path).map_err(|error| io_error(&path, error))?).map_err(
            |error| ManagedModeAddendumError::new(format!("invalid {}: {error}", path.display())),
        )?;
    validate_version(&version)?;
    Ok(version)
}

fn validate_version(version: &ManagedModeAddendumVersion) -> Result<(), ManagedModeAddendumError> {
    if version.version != MANAGED_MODE_ADDENDUM_VERSION {
        return Err(ManagedModeAddendumError::new(format!(
            "unsupported managed mode addendum version {}",
            version.version
        )));
    }
    validate_mode_id(&version.mode_id)?;
    validate_digest(&version.content_digest)?;
    validate_digest(&version.base_digest)?;
    if version.base_version == 0 {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum base version must be positive",
        ));
    }
    if version.source_proposal_id.trim().is_empty()
        || version.source_proposal_id.len() > 128
        || version.source_proposal_id.chars().any(char::is_control)
    {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum source proposal id must be 1-128 non-control characters",
        ));
    }
    validate_changes(&version.changes).and_then(|()| {
            let actual = mode_addendum_content_digest(
                &version.mode_id,
                version.base_version,
                &version.base_digest,
                &version.changes,
            )?;
        if actual != version.content_digest {
                return Err(ManagedModeAddendumError::new(format!(
                    "managed mode addendum {} immutable manifest digest mismatch: expected {}, computed {actual}",
                    version.mode_id, version.content_digest
                )));
        }
        Ok(())
    })
}

fn validate_changes(changes: &[ReviewAddendumChange]) -> Result<(), ManagedModeAddendumError> {
    if changes.is_empty() || changes.len() > crate::MAX_REVIEW_PROPOSAL_CHANGES {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum must contain 1-16 changes",
        ));
    }
    let bytes = serde_json::to_vec(changes)
        .map_err(|error| ManagedModeAddendumError::new(error.to_string()))?;
    if bytes.len() > crate::MAX_REVIEW_METADATA_BYTES {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum metadata is too large",
        ));
    }
    if changes.iter().any(|change| {
        !matches!(
            change.section,
            ReviewAddendumSection::Description | ReviewAddendumSection::RoutingGuidance
        ) || change.after.label.trim().is_empty()
            || change.after.summary.trim().is_empty()
    }) {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum accepts only non-empty description or routing guidance",
        ));
    }
    Ok(())
}

fn validate_mode_id(mode_id: &ModeId) -> Result<(), ManagedModeAddendumError> {
    let value = mode_id.as_str();
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || value.starts_with('.')
        || value.contains("..")
    {
        return Err(ManagedModeAddendumError::new(format!(
            "invalid managed mode id {value}"
        )));
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), ManagedModeAddendumError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum digest must use sha256",
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ManagedModeAddendumError::new(
            "managed mode addendum digest must be 64 lowercase hex characters",
        ));
    }
    Ok(())
}

fn digest_component(digest: &str) -> Result<&str, ManagedModeAddendumError> {
    validate_digest(digest)?;
    Ok(digest.trim_start_matches("sha256:"))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ManagedModeAddendumError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error(path, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error(path, error))?;
    file.sync_all().map_err(|error| io_error(path, error))
}

fn lock_mode(mode_root: &Path) -> Result<fs::File, ManagedModeAddendumError> {
    let path = mode_root.join(".catalog.lock");
    reject_symlink_if_present(&path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error(&path, error))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(|error| {
        ManagedModeAddendumError::new(format!(
            "managed mode addendum lock {}: {error}",
            path.display()
        ))
    })?;
    Ok(file)
}

fn ensure_real_dir(path: &Path) -> Result<(), ManagedModeAddendumError> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    reject_symlink(path)
}

fn sync_directory(path: &Path) -> Result<(), ManagedModeAddendumError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn reject_symlink(path: &Path) -> Result<(), ManagedModeAddendumError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(ManagedModeAddendumError::new(format!(
            "managed mode addendum path {} must not be a symlink",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_if_present(path: &Path) -> Result<(), ManagedModeAddendumError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ManagedModeAddendumError::new(format!(
                "managed mode addendum path {} must not be a symlink",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn io_error(path: &Path, error: std::io::Error) -> ManagedModeAddendumError {
    ManagedModeAddendumError::new(format!(
        "managed mode addendum path {}: {error}",
        path.display()
    ))
}

pub(crate) fn configured_root(root: &Option<PathBuf>) -> Result<&Path, ManagedModeAddendumError> {
    root.as_deref().ok_or_else(|| {
        ManagedModeAddendumError::new("managed mode addendum catalog is not configured")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModesConfig, ReviewMetadata};

    fn raw_digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn proposal_digest(
        mode_id: &ModeId,
        base_digest: &str,
        changes: &[ReviewAddendumChange],
    ) -> String {
        mode_addendum_content_digest(mode_id, 1, base_digest, changes).unwrap()
    }

    fn changes(summary: &str) -> Vec<ReviewAddendumChange> {
        vec![ReviewAddendumChange {
            section: ReviewAddendumSection::Description,
            before: None,
            after: ReviewMetadata {
                label: "Approved guidance".to_string(),
                summary: summary.to_string(),
            },
        }]
    }

    #[test]
    fn versions_compose_without_changing_mode_authority_and_roll_back_to_base() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_mode_addenda_path(temp.path());
        let mode_id = ModeId::new("serious_engineer");
        let base_profile = config.load_assets().profile_or_unknown(&mode_id);
        let first_changes = changes("Prefer explicit verification evidence.");
        let first_base_digest = raw_digest('e');
        let first_digest = proposal_digest(&mode_id, &first_base_digest, &first_changes);
        let first = config
            .install_managed_mode_addendum(ManagedModeAddendumInstall {
                mode_id: mode_id.clone(),
                content_digest: first_digest.clone(),
                base_version: 1,
                base_digest: first_base_digest,
                source_proposal_id: "11111111-1111-1111-1111-111111111111".to_string(),
                expected_active_digest: None,
                changes: first_changes,
            })
            .unwrap();
        assert_eq!(first.previous_digest, None);
        let prompt = config.build_system_prompt(&mode_id, "base", "", "ship this");
        assert!(prompt.system_prompt.contains("Approved mode addendum"));
        assert!(
            prompt
                .system_prompt
                .contains("Prefer explicit verification evidence.")
        );
        assert_eq!(prompt.profile, base_profile);

        let second_changes = changes("Require a focused regression test.");
        let second_base_digest = raw_digest('f');
        let second_digest = proposal_digest(&mode_id, &second_base_digest, &second_changes);
        config
            .install_managed_mode_addendum(ManagedModeAddendumInstall {
                mode_id: mode_id.clone(),
                content_digest: second_digest.clone(),
                base_version: 1,
                base_digest: second_base_digest,
                source_proposal_id: "22222222-2222-2222-2222-222222222222".to_string(),
                expected_active_digest: Some(first_digest.clone()),
                changes: second_changes,
            })
            .unwrap();
        let rolled_back = config
            .rollback_managed_mode_addendum(&mode_id, &second_digest, Some(&first_digest))
            .unwrap();
        assert_eq!(rolled_back.active.unwrap().content_digest, first_digest);
        let base = config
            .rollback_managed_mode_addendum(&mode_id, &first_digest, None)
            .unwrap();
        assert!(base.active.is_none());
        assert!(
            !config
                .build_system_prompt(&mode_id, "base", "", "ship this")
                .system_prompt
                .contains("Approved mode addendum")
        );
        assert_eq!(
            config.load_assets().profile_or_unknown(&mode_id),
            base_profile
        );
    }

    #[test]
    fn stale_activation_and_persona_sections_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_mode_addenda_path(temp.path());
        let mode_id = ModeId::new("general");
        let error = config
            .install_managed_mode_addendum(ManagedModeAddendumInstall {
                mode_id,
                content_digest: raw_digest('c'),
                base_version: 1,
                base_digest: raw_digest('e'),
                source_proposal_id: "33333333-3333-3333-3333-333333333333".to_string(),
                expected_active_digest: Some(raw_digest('d')),
                changes: vec![ReviewAddendumChange {
                    section: ReviewAddendumSection::VoiceGuidance,
                    before: None,
                    after: ReviewMetadata {
                        label: "Unsafe".to_string(),
                        summary: "Try to change voice authority.".to_string(),
                    },
                }],
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("description or routing guidance")
        );
    }

    #[test]
    fn tampered_immutable_manifest_fails_closed_on_reload() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_mode_addenda_path(temp.path());
        let mode_id = ModeId::new("general");
        let base_digest = raw_digest('9');
        let changes = changes("Keep conclusions bounded by evidence.");
        let content_digest = proposal_digest(&mode_id, &base_digest, &changes);
        config
            .install_managed_mode_addendum(ManagedModeAddendumInstall {
                mode_id: mode_id.clone(),
                content_digest: content_digest.clone(),
                base_version: 1,
                base_digest,
                source_proposal_id: "44444444-4444-4444-4444-444444444444".to_string(),
                expected_active_digest: None,
                changes,
            })
            .unwrap();
        let path = temp
            .path()
            .join("general/versions")
            .join(content_digest.trim_start_matches("sha256:"))
            .join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["changes"][0]["after"]["summary"] = serde_json::json!("Tampered guidance.");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        let error = config.active_managed_mode_addendum(&mode_id).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("immutable manifest digest mismatch")
        );
    }
}
