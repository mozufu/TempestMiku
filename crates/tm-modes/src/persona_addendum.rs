use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ReviewAddendumChange, ReviewAddendumSection, ReviewProposalTarget};

const MANAGED_PERSONA_ADDENDUM_VERSION: u16 = 1;
const MIKU_PERSONA_ID: &str = "miku";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPersonaAddendumInstall {
    pub persona_id: String,
    pub content_digest: String,
    pub base_version: u64,
    pub base_digest: String,
    pub source_proposal_id: String,
    pub expected_active_digest: Option<String>,
    pub changes: Vec<ReviewAddendumChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPersonaAddendumVersion {
    pub version: u16,
    pub persona_id: String,
    pub content_digest: String,
    pub base_version: u64,
    pub base_digest: String,
    pub source_proposal_id: String,
    pub changes: Vec<ReviewAddendumChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPersonaAddendumActivation {
    pub active: Option<ManagedPersonaAddendumVersion>,
    pub previous_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPersonaAddendumState {
    pub active: Option<ManagedPersonaAddendumVersion>,
    pub versions: Vec<ManagedPersonaAddendumVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPersonaAddendumError(String);

impl ManagedPersonaAddendumError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ManagedPersonaAddendumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ManagedPersonaAddendumError {}

pub fn persona_addendum_content_digest(
    persona_id: &str,
    base_version: u64,
    base_digest: &str,
    changes: &[ReviewAddendumChange],
) -> Result<String, ManagedPersonaAddendumError> {
    let mut content = serde_json::json!({
        "target": ReviewProposalTarget::Persona { persona_id: persona_id.to_string() },
        "baseVersion": base_version,
        "baseDigest": base_digest,
        "changes": changes,
    });
    canonicalize_json(&mut content);
    let bytes = serde_json::to_vec(&content)
        .map_err(|error| ManagedPersonaAddendumError::new(error.to_string()))?;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_from_digest: Option<String>,
}

pub(crate) fn install(
    root: &Path,
    install: ManagedPersonaAddendumInstall,
) -> Result<ManagedPersonaAddendumActivation, ManagedPersonaAddendumError> {
    validate_persona_id(&install.persona_id)?;
    validate_digest(&install.content_digest)?;
    if let Some(digest) = &install.expected_active_digest {
        validate_digest(digest)?;
    }
    validate_changes(&install.changes)?;
    ensure_real_dir(root)?;
    let persona_root = root.join(&install.persona_id);
    let versions_root = persona_root.join("versions");
    ensure_real_dir(&persona_root)?;
    ensure_real_dir(&versions_root)?;
    let _lock = lock_persona(&persona_root)?;
    let previous_digest =
        read_active_pointer_optional(&persona_root)?.and_then(|pointer| pointer.active_digest);
    let version = ManagedPersonaAddendumVersion {
        version: MANAGED_PERSONA_ADDENDUM_VERSION,
        persona_id: install.persona_id,
        content_digest: install.content_digest,
        base_version: install.base_version,
        base_digest: install.base_digest,
        source_proposal_id: install.source_proposal_id,
        changes: install.changes,
    };
    validate_version(&version)?;
    if previous_digest.as_deref() == Some(version.content_digest.as_str()) {
        let existing = read_version(&persona_root, &version.content_digest)?;
        if existing != version {
            return Err(ManagedPersonaAddendumError::new(format!(
                "managed persona addendum {} is already active with different content or provenance",
                version.persona_id
            )));
        }
        return Ok(ManagedPersonaAddendumActivation {
            active: Some(existing),
            previous_digest: install.expected_active_digest,
        });
    }
    if previous_digest != install.expected_active_digest {
        return Err(ManagedPersonaAddendumError::new(format!(
            "managed persona addendum {} active version changed from {:?} to {:?}",
            version.persona_id, install.expected_active_digest, previous_digest
        )));
    }
    let version_root = versions_root.join(digest_component(&version.content_digest)?);
    install_immutable_version(&versions_root, &version_root, &version)?;
    write_active_pointer(&persona_root, Some(&version.content_digest), None)?;
    Ok(ManagedPersonaAddendumActivation {
        active: Some(version),
        previous_digest,
    })
}

pub(crate) fn rollback(
    root: &Path,
    persona_id: &str,
    expected_active_digest: &str,
    target_digest: Option<&str>,
) -> Result<ManagedPersonaAddendumActivation, ManagedPersonaAddendumError> {
    validate_persona_id(persona_id)?;
    validate_digest(expected_active_digest)?;
    if let Some(digest) = target_digest {
        validate_digest(digest)?;
    }
    let persona_root = root.join(persona_id);
    reject_symlink(&persona_root)?;
    let _lock = lock_persona(&persona_root)?;
    let pointer = read_active_pointer_optional(&persona_root)?.ok_or_else(|| {
        ManagedPersonaAddendumError::new(format!(
            "managed persona addendum {persona_id} has no active pointer"
        ))
    })?;
    if pointer.active_digest.as_deref() == target_digest
        && pointer.rollback_from_digest.as_deref() == Some(expected_active_digest)
    {
        let active = target_digest
            .map(|digest| read_version(&persona_root, digest))
            .transpose()?;
        return Ok(ManagedPersonaAddendumActivation {
            active,
            previous_digest: Some(expected_active_digest.to_string()),
        });
    }
    if pointer.active_digest.as_deref() != Some(expected_active_digest) {
        return Err(ManagedPersonaAddendumError::new(format!(
            "managed persona addendum {persona_id} active version changed from {expected_active_digest} to {:?}",
            pointer.active_digest
        )));
    }
    let active = target_digest
        .map(|digest| read_version(&persona_root, digest))
        .transpose()?;
    write_active_pointer(&persona_root, target_digest, Some(expected_active_digest))?;
    Ok(ManagedPersonaAddendumActivation {
        active,
        previous_digest: Some(expected_active_digest.to_string()),
    })
}

pub(crate) fn active(
    root: &Path,
    persona_id: &str,
) -> Result<Option<ManagedPersonaAddendumVersion>, ManagedPersonaAddendumError> {
    validate_persona_id(persona_id)?;
    let persona_root = root.join(persona_id);
    if !persona_root.exists() {
        return Ok(None);
    }
    let Some(pointer) = read_active_pointer_optional(&persona_root)? else {
        return Ok(None);
    };
    pointer
        .active_digest
        .as_deref()
        .map(|digest| read_version(&persona_root, digest))
        .transpose()
}

pub(crate) fn state(
    root: &Path,
    persona_id: &str,
) -> Result<ManagedPersonaAddendumState, ManagedPersonaAddendumError> {
    validate_persona_id(persona_id)?;
    let persona_root = root.join(persona_id);
    let active = active(root, persona_id)?;
    let versions_root = persona_root.join("versions");
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
                versions.push(read_version(&persona_root, &digest)?);
            }
        }
    }
    versions.sort_by(|left, right| left.content_digest.cmp(&right.content_digest));
    Ok(ManagedPersonaAddendumState { active, versions })
}

fn install_immutable_version(
    versions_root: &Path,
    version_root: &Path,
    version: &ManagedPersonaAddendumVersion,
) -> Result<(), ManagedPersonaAddendumError> {
    if version_root.exists() {
        let existing = read_version_from_root(version_root)?;
        if existing == *version {
            return Ok(());
        }
        return Err(ManagedPersonaAddendumError::new(format!(
            "managed persona addendum version {} already exists with different content",
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
            .map_err(|error| ManagedPersonaAddendumError::new(error.to_string()))?;
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
    persona_root: &Path,
    active_digest: Option<&str>,
    rollback_from_digest: Option<&str>,
) -> Result<(), ManagedPersonaAddendumError> {
    let bytes = serde_json::to_vec_pretty(&ActivePointer {
        active_digest: active_digest.map(str::to_string),
        rollback_from_digest: rollback_from_digest.map(str::to_string),
    })
    .map_err(|error| ManagedPersonaAddendumError::new(error.to_string()))?;
    let path = persona_root.join("active.json");
    let temp = persona_root.join(format!(
        ".active-{}-{}.tmp",
        std::process::id(),
        TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    write_new_file(&temp, &bytes)?;
    let result = fs::rename(&temp, &path)
        .map_err(|error| io_error(&path, error))
        .and_then(|()| sync_directory(persona_root));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn read_active_pointer_optional(
    persona_root: &Path,
) -> Result<Option<ActivePointer>, ManagedPersonaAddendumError> {
    let path = persona_root.join("active.json");
    reject_symlink_if_present(&path)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(&path, error)),
    };
    let pointer: ActivePointer = serde_json::from_slice(&bytes).map_err(|error| {
        ManagedPersonaAddendumError::new(format!("invalid {}: {error}", path.display()))
    })?;
    if let Some(digest) = &pointer.active_digest {
        validate_digest(digest)?;
    }
    if let Some(digest) = &pointer.rollback_from_digest {
        validate_digest(digest)?;
    }
    Ok(Some(pointer))
}

fn read_version(
    persona_root: &Path,
    digest: &str,
) -> Result<ManagedPersonaAddendumVersion, ManagedPersonaAddendumError> {
    let root = persona_root
        .join("versions")
        .join(digest_component(digest)?);
    let version = read_version_from_root(&root)?;
    let expected_persona = persona_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ManagedPersonaAddendumError::new("managed persona addendum root has no valid name")
        })?;
    if version.persona_id != expected_persona || version.content_digest != digest {
        return Err(ManagedPersonaAddendumError::new(format!(
            "managed persona addendum version path does not match manifest for {digest}"
        )));
    }
    Ok(version)
}

fn read_version_from_root(
    root: &Path,
) -> Result<ManagedPersonaAddendumVersion, ManagedPersonaAddendumError> {
    reject_symlink(root)?;
    let path = root.join("manifest.json");
    reject_symlink(&path)?;
    let version: ManagedPersonaAddendumVersion = serde_json::from_slice(
        &fs::read(&path).map_err(|error| io_error(&path, error))?,
    )
    .map_err(|error| {
        ManagedPersonaAddendumError::new(format!("invalid {}: {error}", path.display()))
    })?;
    validate_version(&version)?;
    Ok(version)
}

fn validate_version(
    version: &ManagedPersonaAddendumVersion,
) -> Result<(), ManagedPersonaAddendumError> {
    if version.version != MANAGED_PERSONA_ADDENDUM_VERSION {
        return Err(ManagedPersonaAddendumError::new(format!(
            "unsupported managed persona addendum version {}",
            version.version
        )));
    }
    validate_persona_id(&version.persona_id)?;
    validate_digest(&version.content_digest)?;
    validate_digest(&version.base_digest)?;
    if version.base_version == 0 {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum base version must be positive",
        ));
    }
    if version.source_proposal_id.trim().is_empty()
        || version.source_proposal_id.len() > 128
        || version.source_proposal_id.chars().any(char::is_control)
    {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum source proposal id must be 1-128 non-control characters",
        ));
    }
    validate_changes(&version.changes)?;
    let actual = persona_addendum_content_digest(
        &version.persona_id,
        version.base_version,
        &version.base_digest,
        &version.changes,
    )?;
    if actual != version.content_digest {
        return Err(ManagedPersonaAddendumError::new(format!(
            "managed persona addendum {} immutable manifest digest mismatch: expected {}, computed {actual}",
            version.persona_id, version.content_digest
        )));
    }
    Ok(())
}

fn validate_changes(changes: &[ReviewAddendumChange]) -> Result<(), ManagedPersonaAddendumError> {
    if changes.is_empty() || changes.len() > crate::MAX_REVIEW_PROPOSAL_CHANGES {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum must contain 1-16 changes",
        ));
    }
    let bytes = serde_json::to_vec(changes)
        .map_err(|error| ManagedPersonaAddendumError::new(error.to_string()))?;
    if bytes.len() > crate::MAX_REVIEW_METADATA_BYTES {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum metadata is too large",
        ));
    }
    if changes.iter().any(|change| {
        !matches!(
            change.section,
            ReviewAddendumSection::ToneGuidance
                | ReviewAddendumSection::AddressGuidance
                | ReviewAddendumSection::InteractionPreference
        ) || change.after.label.trim().is_empty()
            || change.after.summary.trim().is_empty()
    }) {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum accepts only non-empty tone, address, or interaction preference guidance",
        ));
    }
    Ok(())
}

fn validate_persona_id(persona_id: &str) -> Result<(), ManagedPersonaAddendumError> {
    if persona_id != MIKU_PERSONA_ID {
        return Err(ManagedPersonaAddendumError::new(format!(
            "unknown managed persona addendum target {persona_id}"
        )));
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), ManagedPersonaAddendumError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum digest must use sha256",
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ManagedPersonaAddendumError::new(
            "managed persona addendum digest must be 64 lowercase hex characters",
        ));
    }
    Ok(())
}

fn digest_component(digest: &str) -> Result<&str, ManagedPersonaAddendumError> {
    validate_digest(digest)?;
    Ok(digest.trim_start_matches("sha256:"))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ManagedPersonaAddendumError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error(path, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error(path, error))?;
    file.sync_all().map_err(|error| io_error(path, error))
}

fn lock_persona(persona_root: &Path) -> Result<fs::File, ManagedPersonaAddendumError> {
    let path = persona_root.join(".catalog.lock");
    reject_symlink_if_present(&path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error(&path, error))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(|error| {
        ManagedPersonaAddendumError::new(format!(
            "managed persona addendum lock {}: {error}",
            path.display()
        ))
    })?;
    Ok(file)
}

fn ensure_real_dir(path: &Path) -> Result<(), ManagedPersonaAddendumError> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    reject_symlink(path)
}

fn sync_directory(path: &Path) -> Result<(), ManagedPersonaAddendumError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn reject_symlink(path: &Path) -> Result<(), ManagedPersonaAddendumError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(ManagedPersonaAddendumError::new(format!(
            "managed persona addendum path {} must not be a symlink",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_if_present(path: &Path) -> Result<(), ManagedPersonaAddendumError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ManagedPersonaAddendumError::new(format!(
                "managed persona addendum path {} must not be a symlink",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn io_error(path: &Path, error: std::io::Error) -> ManagedPersonaAddendumError {
    ManagedPersonaAddendumError::new(format!(
        "managed persona addendum path {}: {error}",
        path.display()
    ))
}

pub(crate) fn configured_root(
    root: &Option<PathBuf>,
) -> Result<&Path, ManagedPersonaAddendumError> {
    root.as_deref().ok_or_else(|| {
        ManagedPersonaAddendumError::new("managed persona addendum catalog is not configured")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModeId, ModesConfig, ReviewMetadata};

    fn raw_digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn changes(summary: &str) -> Vec<ReviewAddendumChange> {
        vec![ReviewAddendumChange {
            section: ReviewAddendumSection::InteractionPreference,
            before: None,
            after: ReviewMetadata {
                label: "Approved interaction preference".to_string(),
                summary: summary.to_string(),
            },
        }]
    }

    #[test]
    fn versions_compose_for_every_mode_and_roll_back_to_hand_authored_base() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_persona_addenda_path(temp.path());
        let base_digest = raw_digest('e');
        let first_changes = changes("Address the owner as Brian when a name is useful.");
        let content_digest =
            persona_addendum_content_digest("miku", 1, &base_digest, &first_changes).unwrap();
        config
            .install_managed_persona_addendum(ManagedPersonaAddendumInstall {
                persona_id: "miku".to_string(),
                content_digest: content_digest.clone(),
                base_version: 1,
                base_digest,
                source_proposal_id: "11111111-1111-1111-1111-111111111111".to_string(),
                expected_active_digest: None,
                changes: first_changes,
            })
            .unwrap();
        for mode in [ModeId::new("general"), ModeId::new("serious_engineer")] {
            let prompt = config.build_system_prompt(
                &mode,
                "base",
                "",
                "help",
                &std::collections::BTreeSet::new(),
            );
            assert!(prompt.system_prompt.contains("Approved persona addendum"));
            assert!(prompt.system_prompt.contains("Address the owner as Brian"));
        }
        let second_base_digest = raw_digest('f');
        let second_changes = changes("Lead with the verified outcome.");
        let second_digest =
            persona_addendum_content_digest("miku", 1, &second_base_digest, &second_changes)
                .unwrap();
        config
            .install_managed_persona_addendum(ManagedPersonaAddendumInstall {
                persona_id: "miku".to_string(),
                content_digest: second_digest.clone(),
                base_version: 1,
                base_digest: second_base_digest,
                source_proposal_id: "22222222-2222-2222-2222-222222222222".to_string(),
                expected_active_digest: Some(content_digest.clone()),
                changes: second_changes,
            })
            .unwrap();
        let prior = config
            .rollback_managed_persona_addendum("miku", &second_digest, Some(&content_digest))
            .unwrap();
        assert_eq!(prior.active.unwrap().content_digest, content_digest);
        config
            .rollback_managed_persona_addendum("miku", &content_digest, None)
            .unwrap();
        assert!(
            !config
                .build_system_prompt(
                    &ModeId::new("general"),
                    "base",
                    "",
                    "help",
                    &std::collections::BTreeSet::new(),
                )
                .system_prompt
                .contains("Approved persona addendum")
        );
    }

    #[test]
    fn mode_or_legacy_persona_sections_fail_closed() {
        for section in [
            ReviewAddendumSection::Description,
            ReviewAddendumSection::BehaviorGuidance,
            ReviewAddendumSection::VoiceGuidance,
        ] {
            let error = validate_changes(&[ReviewAddendumChange {
                section,
                before: None,
                after: ReviewMetadata {
                    label: "Not activatable".to_string(),
                    summary: "Must remain review-only.".to_string(),
                },
            }])
            .unwrap_err();
            assert!(error.to_string().contains("tone, address, or interaction"));
        }
    }

    #[test]
    fn stale_activation_and_tampered_immutable_manifest_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_persona_addenda_path(temp.path());
        let base_digest = raw_digest('a');
        let changes = changes("Keep the response concise.");
        let content_digest =
            persona_addendum_content_digest("miku", 1, &base_digest, &changes).unwrap();
        let stale = config
            .install_managed_persona_addendum(ManagedPersonaAddendumInstall {
                persona_id: "miku".to_string(),
                content_digest: content_digest.clone(),
                base_version: 1,
                base_digest: base_digest.clone(),
                source_proposal_id: "33333333-3333-3333-3333-333333333333".to_string(),
                expected_active_digest: Some(raw_digest('b')),
                changes: changes.clone(),
            })
            .unwrap_err();
        assert!(stale.to_string().contains("active version changed"));

        config
            .install_managed_persona_addendum(ManagedPersonaAddendumInstall {
                persona_id: "miku".to_string(),
                content_digest: content_digest.clone(),
                base_version: 1,
                base_digest,
                source_proposal_id: "33333333-3333-3333-3333-333333333333".to_string(),
                expected_active_digest: None,
                changes,
            })
            .unwrap();
        let path = temp
            .path()
            .join("miku/versions")
            .join(content_digest.trim_start_matches("sha256:"))
            .join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["changes"][0]["after"]["summary"] =
            serde_json::json!("Tampered persona guidance.");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        let error = config.active_managed_persona_addendum("miku").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("immutable manifest digest mismatch")
        );
    }

    #[test]
    fn install_and_rollback_retries_are_idempotent_but_divergent_provenance_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_persona_addenda_path(temp.path());
        let base_digest = raw_digest('7');
        let first_changes = changes("Prefer one concrete next step.");
        let content_digest =
            persona_addendum_content_digest("miku", 1, &base_digest, &first_changes).unwrap();
        let install = ManagedPersonaAddendumInstall {
            persona_id: "miku".to_string(),
            content_digest: content_digest.clone(),
            base_version: 1,
            base_digest,
            source_proposal_id: "55555555-5555-5555-5555-555555555555".to_string(),
            expected_active_digest: None,
            changes: first_changes,
        };
        let first = config
            .install_managed_persona_addendum(install.clone())
            .unwrap();
        let retry = config
            .install_managed_persona_addendum(install.clone())
            .unwrap();
        assert_eq!(retry, first);

        let mut divergent = install;
        divergent.source_proposal_id = "66666666-6666-6666-6666-666666666666".to_string();
        let error = config
            .install_managed_persona_addendum(divergent)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("different content or provenance")
        );

        let second_base_digest = raw_digest('9');
        let second_changes = changes("Keep the conclusion bounded by evidence.");
        let second_digest =
            persona_addendum_content_digest("miku", 1, &second_base_digest, &second_changes)
                .unwrap();
        config
            .install_managed_persona_addendum(ManagedPersonaAddendumInstall {
                persona_id: "miku".to_string(),
                content_digest: second_digest.clone(),
                base_version: 1,
                base_digest: second_base_digest,
                source_proposal_id: "77777777-7777-7777-7777-777777777777".to_string(),
                expected_active_digest: Some(content_digest.clone()),
                changes: second_changes,
            })
            .unwrap();
        let first_rollback = config
            .rollback_managed_persona_addendum("miku", &second_digest, Some(&content_digest))
            .unwrap();
        let retry_rollback = config
            .rollback_managed_persona_addendum("miku", &second_digest, Some(&content_digest))
            .unwrap();
        assert_eq!(retry_rollback, first_rollback);
        let error = config
            .rollback_managed_persona_addendum("miku", &raw_digest('8'), Some(&content_digest))
            .unwrap_err();
        assert!(error.to_string().contains("active version changed"));

        let first_rollback = config
            .rollback_managed_persona_addendum("miku", &content_digest, None)
            .unwrap();
        let retry_rollback = config
            .rollback_managed_persona_addendum("miku", &content_digest, None)
            .unwrap();
        assert_eq!(retry_rollback, first_rollback);

        let error = config
            .rollback_managed_persona_addendum("miku", &raw_digest('6'), None)
            .unwrap_err();
        assert!(error.to_string().contains("active version changed"));
    }
}
