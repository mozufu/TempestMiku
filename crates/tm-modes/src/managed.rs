use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MANAGED_SKILL_VERSION: u16 = 1;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;
const MAX_USE_CRITERIA_BYTES: usize = 8 * 1024;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSkillInstall {
    pub name: String,
    pub body: String,
    pub content_digest: String,
    pub source_proposal_id: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub use_criteria: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSkillVersion {
    pub version: u16,
    pub name: String,
    pub content_digest: String,
    pub source_proposal_id: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub use_criteria: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSkillActivation {
    pub active: ManagedSkillVersion,
    pub previous_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSkillState {
    pub active: ManagedSkillVersion,
    pub versions: Vec<ManagedSkillVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSkillError(String);

impl ManagedSkillError {
    pub(crate) fn from_message(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn new(message: impl Into<String>) -> Self {
        Self::from_message(message)
    }
}

impl fmt::Display for ManagedSkillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ManagedSkillError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivePointer {
    active_digest: String,
}

pub(crate) fn install(
    root: &Path,
    install: ManagedSkillInstall,
) -> Result<ManagedSkillActivation, ManagedSkillError> {
    validate_name(&install.name)?;
    validate_digest(&install.content_digest)?;
    if body_digest(&install.body) != install.content_digest {
        return Err(ManagedSkillError::new(format!(
            "managed skill {} body digest does not match {}",
            install.name, install.content_digest
        )));
    }
    let triggers = install
        .triggers
        .into_iter()
        .map(|trigger| trigger.trim().to_string())
        .filter(|trigger| !trigger.is_empty())
        .collect::<Vec<_>>();

    ensure_real_dir(root)?;
    let skill_root = root.join(&install.name);
    let versions_root = skill_root.join("versions");
    ensure_real_dir(&skill_root)?;
    ensure_real_dir(&versions_root)?;
    let _lock = lock_skill(&skill_root)?;

    let version = ManagedSkillVersion {
        version: MANAGED_SKILL_VERSION,
        name: install.name,
        content_digest: install.content_digest,
        source_proposal_id: install.source_proposal_id,
        description: install.description,
        triggers,
        use_criteria: install.use_criteria,
    };
    validate_version_metadata(&version)?;
    if install.body.len() > MAX_BODY_BYTES {
        return Err(ManagedSkillError::new(format!(
            "managed skill body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    let version_root = versions_root.join(digest_component(&version.content_digest)?);
    install_immutable_version(&versions_root, &version_root, &version, &install.body)?;

    let previous_digest = read_active_pointer(&skill_root)
        .ok()
        .map(|pointer| pointer.active_digest);
    write_active_pointer(&skill_root, &version.content_digest)?;
    Ok(ManagedSkillActivation {
        active: version,
        previous_digest,
    })
}

pub(crate) fn rollback(
    root: &Path,
    name: &str,
    expected_active_digest: &str,
    target_digest: &str,
) -> Result<ManagedSkillActivation, ManagedSkillError> {
    validate_name(name)?;
    validate_digest(expected_active_digest)?;
    validate_digest(target_digest)?;
    let skill_root = root.join(name);
    reject_symlink(&skill_root)?;
    let _lock = lock_skill(&skill_root)?;
    let pointer = read_active_pointer(&skill_root)?;
    if pointer.active_digest != expected_active_digest {
        return Err(ManagedSkillError::new(format!(
            "managed skill {name} active version changed from {expected_active_digest} to {}",
            pointer.active_digest
        )));
    }
    let target = read_version(&skill_root, target_digest)?.0;
    write_active_pointer(&skill_root, target_digest)?;
    Ok(ManagedSkillActivation {
        active: target,
        previous_digest: Some(expected_active_digest.to_string()),
    })
}

pub(crate) fn state(root: &Path, name: &str) -> Result<ManagedSkillState, ManagedSkillError> {
    validate_name(name)?;
    let skill_root = root.join(name);
    let active = read_version(
        &skill_root,
        &read_active_pointer(&skill_root)?.active_digest,
    )?
    .0;
    let mut versions = Vec::new();
    let versions_root = skill_root.join("versions");
    for entry in fs::read_dir(&versions_root).map_err(|error| io_error(&versions_root, error))? {
        let entry = entry.map_err(|error| io_error(&versions_root, error))?;
        if entry
            .file_type()
            .map_err(|error| io_error(&entry.path(), error))?
            .is_dir()
        {
            let digest = format!("sha256:{}", entry.file_name().to_string_lossy());
            versions.push(read_version(&skill_root, &digest)?.0);
        }
    }
    versions.sort_by(|left, right| left.content_digest.cmp(&right.content_digest));
    Ok(ManagedSkillState { active, versions })
}

pub(crate) fn states(root: &Path) -> Result<Vec<ManagedSkillState>, ManagedSkillError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    reject_symlink(root)?;
    let mut states = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| io_error(root, error))? {
        let entry = entry.map_err(|error| io_error(root, error))?;
        if entry
            .file_type()
            .map_err(|error| io_error(&entry.path(), error))?
            .is_dir()
        {
            let name = entry.file_name().to_string_lossy().to_string();
            states.push(state(root, &name)?);
        }
    }
    states.sort_by(|left, right| left.active.name.cmp(&right.active.name));
    Ok(states)
}

pub(crate) fn active_body(
    root: &Path,
    name: &str,
) -> Result<(ManagedSkillVersion, String), ManagedSkillError> {
    validate_name(name)?;
    let skill_root = root.join(name);
    let pointer = read_active_pointer(&skill_root)?;
    read_version(&skill_root, &pointer.active_digest)
}

pub(crate) fn version_body(
    root: &Path,
    name: &str,
    digest: &str,
) -> Result<(ManagedSkillVersion, String), ManagedSkillError> {
    validate_name(name)?;
    read_version(&root.join(name), digest)
}

fn install_immutable_version(
    versions_root: &Path,
    version_root: &Path,
    version: &ManagedSkillVersion,
    body: &str,
) -> Result<(), ManagedSkillError> {
    if version_root.exists() {
        let (existing, existing_body) = read_version_from_root(version_root)?;
        if existing == *version && existing_body == body {
            return Ok(());
        }
        return Err(ManagedSkillError::new(format!(
            "managed skill version {} already exists with different content",
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
        write_new_file(&temp_root.join("SKILL.md"), body.as_bytes())?;
        let manifest = serde_json::to_vec_pretty(version)
            .map_err(|error| ManagedSkillError::new(error.to_string()))?;
        write_new_file(&temp_root.join("manifest.json"), &manifest)?;
        fs::rename(&temp_root, version_root).map_err(|error| io_error(version_root, error))?;
        sync_directory(versions_root)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp_root);
    }
    result
}

fn write_active_pointer(skill_root: &Path, digest: &str) -> Result<(), ManagedSkillError> {
    let pointer = serde_json::to_vec_pretty(&ActivePointer {
        active_digest: digest.to_string(),
    })
    .map_err(|error| ManagedSkillError::new(error.to_string()))?;
    let path = skill_root.join("active.json");
    let temp = skill_root.join(format!(
        ".active-{}-{}.tmp",
        std::process::id(),
        TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    write_new_file(&temp, &pointer)?;
    let result = fs::rename(&temp, &path)
        .map_err(|error| io_error(&path, error))
        .and_then(|()| sync_directory(skill_root));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn read_active_pointer(skill_root: &Path) -> Result<ActivePointer, ManagedSkillError> {
    let path = skill_root.join("active.json");
    reject_symlink(&path)?;
    let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
    let pointer: ActivePointer = serde_json::from_slice(&bytes)
        .map_err(|error| ManagedSkillError::new(format!("invalid {}: {error}", path.display())))?;
    validate_digest(&pointer.active_digest)?;
    Ok(pointer)
}

fn read_version(
    skill_root: &Path,
    digest: &str,
) -> Result<(ManagedSkillVersion, String), ManagedSkillError> {
    let root = skill_root.join("versions").join(digest_component(digest)?);
    let (version, body) = read_version_from_root(&root)?;
    let expected_name = skill_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ManagedSkillError::new("managed skill root has no valid name"))?;
    if version.name != expected_name || version.content_digest != digest {
        return Err(ManagedSkillError::new(format!(
            "managed skill version path does not match manifest for {digest}"
        )));
    }
    Ok((version, body))
}

fn read_version_from_root(root: &Path) -> Result<(ManagedSkillVersion, String), ManagedSkillError> {
    reject_symlink(root)?;
    let manifest_path = root.join("manifest.json");
    let body_path = root.join("SKILL.md");
    reject_symlink(&manifest_path)?;
    reject_symlink(&body_path)?;
    let manifest: ManagedSkillVersion = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|error| io_error(&manifest_path, error))?,
    )
    .map_err(|error| {
        ManagedSkillError::new(format!("invalid {}: {error}", manifest_path.display()))
    })?;
    validate_version_metadata(&manifest)?;
    let body = fs::read_to_string(&body_path).map_err(|error| io_error(&body_path, error))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ManagedSkillError::new(format!(
            "managed skill body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    if body_digest(&body) != manifest.content_digest {
        return Err(ManagedSkillError::new(format!(
            "managed skill {} immutable body digest mismatch",
            manifest.name
        )));
    }
    Ok((manifest, body))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ManagedSkillError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error(path, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error(path, error))?;
    file.sync_all().map_err(|error| io_error(path, error))
}

fn lock_skill(skill_root: &Path) -> Result<fs::File, ManagedSkillError> {
    let path = skill_root.join(".catalog.lock");
    reject_symlink_if_present(&path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error(&path, error))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(|error| {
        ManagedSkillError::new(format!("managed skill lock {}: {error}", path.display()))
    })?;
    Ok(file)
}

fn sync_directory(path: &Path) -> Result<(), ManagedSkillError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn ensure_real_dir(path: &Path) -> Result<(), ManagedSkillError> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    reject_symlink(path)
}

fn reject_symlink(path: &Path) -> Result<(), ManagedSkillError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(ManagedSkillError::new(format!(
            "managed skill path {} must not be a symlink",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_if_present(path: &Path) -> Result<(), ManagedSkillError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ManagedSkillError::new(format!(
            "managed skill path {} must not be a symlink",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn validate_version_metadata(version: &ManagedSkillVersion) -> Result<(), ManagedSkillError> {
    if version.version != MANAGED_SKILL_VERSION {
        return Err(ManagedSkillError::new(format!(
            "unsupported managed skill version {}",
            version.version
        )));
    }
    validate_name(&version.name)?;
    validate_digest(&version.content_digest)?;
    if version.source_proposal_id.trim().is_empty()
        || version.source_proposal_id.len() > 128
        || version.source_proposal_id.chars().any(char::is_control)
    {
        return Err(ManagedSkillError::new(
            "managed skill source proposal id must be 1-128 non-control characters",
        ));
    }
    if version.description.trim().is_empty()
        || version.description.len() > MAX_DESCRIPTION_BYTES
        || version.description.chars().any(char::is_control)
    {
        return Err(ManagedSkillError::new(format!(
            "managed skill description must be 1-{MAX_DESCRIPTION_BYTES} non-control bytes"
        )));
    }
    if version.triggers.is_empty()
        || version.triggers.len() > 16
        || version.triggers.iter().any(|item| {
            item.trim().is_empty() || item.len() > 512 || item.chars().any(char::is_control)
        })
    {
        return Err(ManagedSkillError::new(
            "managed skill must have 1-16 non-control triggers of at most 512 bytes",
        ));
    }
    if version.use_criteria.trim().is_empty()
        || version.use_criteria.len() > MAX_USE_CRITERIA_BYTES
        || version.use_criteria.chars().any(char::is_control)
    {
        return Err(ManagedSkillError::new(format!(
            "managed skill use criteria must be 1-{MAX_USE_CRITERIA_BYTES} non-control bytes"
        )));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ManagedSkillError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
    {
        return Err(ManagedSkillError::new(format!(
            "invalid managed skill name {name}"
        )));
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), ManagedSkillError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(ManagedSkillError::new(
            "managed skill digest must use sha256",
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ManagedSkillError::new(
            "managed skill digest must be 64 lowercase hex characters",
        ));
    }
    Ok(())
}

fn digest_component(digest: &str) -> Result<&str, ManagedSkillError> {
    validate_digest(digest)?;
    Ok(digest.trim_start_matches("sha256:"))
}

fn body_digest(body: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(body.as_bytes()))
}

fn io_error(path: &Path, error: std::io::Error) -> ManagedSkillError {
    ManagedSkillError::new(format!("managed skill path {}: {error}", path.display()))
}

pub(crate) fn configured_root(root: &Option<PathBuf>) -> Result<&Path, ManagedSkillError> {
    root.as_deref()
        .ok_or_else(|| ManagedSkillError::new("managed skill catalog is not configured"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModeId, ModesConfig};

    fn install(name: &str, body: &str, proposal: &str) -> ManagedSkillInstall {
        ManagedSkillInstall {
            name: name.to_string(),
            body: body.to_string(),
            content_digest: body_digest(body),
            source_proposal_id: proposal.to_string(),
            description: format!("{name} description"),
            triggers: vec!["release notes".to_string()],
            use_criteria: "Use for the recurring workflow.".to_string(),
        }
    }

    #[test]
    fn install_reload_upgrade_and_rollback_are_versioned() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_skills_path(temp.path());
        let first_body = "# release-workflow\n\nfirst version\n";
        let first = config
            .install_managed_skill(install(
                "release-workflow",
                first_body,
                "00000000-0000-0000-0000-000000000001",
            ))
            .unwrap();
        assert_eq!(first.previous_digest, None);

        let prompt = config.build_system_prompt_with_managed_snapshot(
            &ModeId::from("general"),
            "base",
            "",
            "please draft release notes",
            &std::collections::BTreeSet::new(),
            &[crate::ManagedSkillPromptSnapshot {
                version: first.active.clone(),
                body: first_body.to_string(),
            }],
        );
        assert!(prompt.system_prompt.contains("first version"));
        let ungoverned = config.build_system_prompt(
            &ModeId::from("general"),
            "base",
            "",
            "please draft release notes",
            &std::collections::BTreeSet::new(),
        );
        assert!(!ungoverned.system_prompt.contains("first version"));

        let second_body = "# release-workflow\n\nsecond version\n";
        let second = config
            .install_managed_skill(install(
                "release-workflow",
                second_body,
                "00000000-0000-0000-0000-000000000002",
            ))
            .unwrap();
        assert_eq!(
            second.previous_digest.as_deref(),
            Some(first.active.content_digest.as_str())
        );
        assert_eq!(
            config
                .managed_skill("release-workflow")
                .unwrap()
                .versions
                .len(),
            2
        );

        let rolled_back = config
            .rollback_managed_skill(
                "release-workflow",
                &second.active.content_digest,
                &first.active.content_digest,
            )
            .unwrap();
        assert_eq!(
            rolled_back.active.content_digest,
            first.active.content_digest
        );

        let restarted = ModesConfig::default().with_managed_skills_path(temp.path());
        let (_, body) = restarted.managed_skill_body("release-workflow").unwrap();
        assert_eq!(body, first_body);
    }

    #[test]
    fn hand_authored_collision_and_stale_rollback_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_skills_path(temp.path());
        assert!(
            config
                .install_managed_skill(install(
                    "scope-guard",
                    "# scope-guard\n\nreplacement\n",
                    "00000000-0000-0000-0000-000000000001",
                ))
                .unwrap_err()
                .to_string()
                .contains("collides")
        );

        let active = config
            .install_managed_skill(install(
                "release-workflow",
                "# release-workflow\n\nfirst\n",
                "00000000-0000-0000-0000-000000000001",
            ))
            .unwrap();
        assert!(
            config
                .rollback_managed_skill(
                    "release-workflow",
                    &format!("sha256:{}", "0".repeat(64)),
                    &active.active.content_digest,
                )
                .unwrap_err()
                .to_string()
                .contains("active version changed")
        );
    }

    #[test]
    fn swapped_version_directory_content_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = ModesConfig::default().with_managed_skills_path(temp.path());
        let first = config
            .install_managed_skill(install(
                "release-workflow",
                "# release-workflow\n\nfirst\n",
                "00000000-0000-0000-0000-000000000001",
            ))
            .unwrap();
        let second = config
            .install_managed_skill(install(
                "release-workflow",
                "# release-workflow\n\nsecond\n",
                "00000000-0000-0000-0000-000000000002",
            ))
            .unwrap();
        let versions = temp.path().join("release-workflow/versions");
        let first_root = versions.join(first.active.content_digest.trim_start_matches("sha256:"));
        let second_root = versions.join(second.active.content_digest.trim_start_matches("sha256:"));
        fs::copy(first_root.join("SKILL.md"), second_root.join("SKILL.md")).unwrap();
        fs::copy(
            first_root.join("manifest.json"),
            second_root.join("manifest.json"),
        )
        .unwrap();

        assert!(
            config
                .managed_skill_body("release-workflow")
                .unwrap_err()
                .to_string()
                .contains("path does not match manifest")
        );
    }
}
