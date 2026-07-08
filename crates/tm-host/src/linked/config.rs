use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};
use tm_artifacts::{ResourceContent, preview};

use crate::{HostError, Result};

use super::util::{
    ensure_existing_ancestor_under_root, ensure_under_root, linked_uri, parse_linked_path,
    select_text, validate_alias, validate_command_name,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct P0HostConfig {
    #[serde(default)]
    pub linked_folders: Vec<LinkedFolderConfig>,
    #[serde(default)]
    pub approvals: ApprovalConfig,
    #[serde(default)]
    pub artifact_root: Option<PathBuf>,
}

impl P0HostConfig {
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content =
            fs::read_to_string(path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let config: Self = serde_json::from_str(&content)
            .map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        LinkedFolders::from_configs(self.linked_folders.clone()).map(|_| ())
    }

    pub fn linked_folders(&self) -> Result<LinkedFolders> {
        LinkedFolders::from_configs(self.linked_folders.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LinkedFolderConfig {
    pub name: String,
    pub path: PathBuf,
    pub mode: FsMode,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub safe_args: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsMode {
    Ro,
    Rw,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ApprovalConfig {
    #[serde(default = "default_approval_mode")]
    pub mode: String,
    #[serde(default = "default_approval_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            mode: default_approval_mode(),
            timeout_ms: default_approval_timeout_ms(),
        }
    }
}

pub fn default_approval_mode() -> String {
    "deny".to_string()
}

pub fn default_approval_timeout_ms() -> u64 {
    60_000
}

#[derive(Debug, Clone)]
pub struct FsPolicy {
    pub alias: String,
    pub root: PathBuf,
    pub mode: FsMode,
    pub commands: BTreeSet<String>,
    pub safe_args: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct LinkedFolders {
    policies: Arc<RwLock<BTreeMap<String, FsPolicy>>>,
}

impl LinkedFolders {
    pub fn from_configs(configs: Vec<LinkedFolderConfig>) -> Result<Self> {
        let mut policies = BTreeMap::new();
        for config in configs {
            if policies.contains_key(&config.name) {
                return Err(HostError::InvalidArgs(format!(
                    "duplicate linked folder alias {}",
                    config.name
                )));
            }
            let policy = policy_from_config(config)?;
            policies.insert(policy.alias.clone(), policy);
        }
        Ok(Self {
            policies: Arc::new(RwLock::new(policies)),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.policies
            .read()
            .map(|policies| policies.is_empty())
            .unwrap_or(true)
    }

    pub fn first_alias(&self) -> Option<String> {
        self.policies
            .read()
            .ok()
            .and_then(|policies| policies.keys().next().cloned())
    }

    pub fn policies(&self) -> Vec<FsPolicy> {
        self.policies
            .read()
            .map(|policies| policies.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn policy(&self, alias: &str) -> Result<FsPolicy> {
        self.policies
            .read()
            .map_err(|err| HostError::HostCall(format!("linked folder registry poisoned: {err}")))?
            .get(alias)
            .cloned()
            .ok_or_else(|| HostError::InvalidPath(format!("unknown linked folder alias {alias}")))
    }

    pub fn insert_policy(&self, policy: FsPolicy) -> Result<FsPolicy> {
        let policy = validate_policy(policy)?;
        let mut policies = self.policies.write().map_err(|err| {
            HostError::HostCall(format!("linked folder registry poisoned: {err}"))
        })?;
        if let Some(existing) = policies.get(&policy.alias) {
            if existing.root == policy.root && existing.mode == policy.mode {
                return Ok(existing.clone());
            }
            if existing.root == policy.root {
                policies.insert(policy.alias.clone(), policy.clone());
                return Ok(policy);
            }
            return Err(HostError::InvalidArgs(format!(
                "duplicate linked folder alias {}",
                policy.alias
            )));
        }
        policies.insert(policy.alias.clone(), policy.clone());
        Ok(policy)
    }

    pub fn remove_policy(&self, alias: &str) -> Result<FsPolicy> {
        validate_alias(alias)?;
        self.policies
            .write()
            .map_err(|err| HostError::HostCall(format!("linked folder registry poisoned: {err}")))?
            .remove(alias)
            .ok_or_else(|| HostError::InvalidPath(format!("unknown linked folder alias {alias}")))
    }

    pub(super) fn resolve_existing(&self, input: Option<&str>) -> Result<ResolvedPath> {
        let parsed = self.parse_path(input)?;
        let policy = self.policy(&parsed.alias)?;
        let joined = policy.root.join(&parsed.relative);
        let path = joined
            .canonicalize()
            .map_err(|err| HostError::InvalidPath(format!("{}: {err}", parsed.display)))?;
        ensure_under_root(&path, &policy.root, &parsed.display)?;
        Ok(ResolvedPath {
            alias: parsed.alias,
            relative: parsed.relative,
            display: parsed.display,
            path,
            policy,
        })
    }

    pub(super) fn resolve_for_create(
        &self,
        input: &str,
        create_parents: bool,
    ) -> Result<ResolvedPath> {
        let parsed = self.parse_path(Some(input))?;
        let policy = self.policy(&parsed.alias)?;
        let target = policy.root.join(&parsed.relative);
        if target.exists() {
            let path = target
                .canonicalize()
                .map_err(|err| HostError::InvalidPath(format!("{}: {err}", parsed.display)))?;
            ensure_under_root(&path, &policy.root, &parsed.display)?;
            return Ok(ResolvedPath {
                alias: parsed.alias,
                relative: parsed.relative,
                display: parsed.display,
                path,
                policy,
            });
        }
        let parent = target.parent().ok_or_else(|| {
            HostError::InvalidPath(format!("missing parent for {}", parsed.display))
        })?;
        ensure_existing_ancestor_under_root(parent, &policy.root, &parsed.display)?;
        if !parent.exists() {
            if !create_parents {
                return Err(HostError::InvalidPath(format!(
                    "parent does not exist for {}",
                    parsed.display
                )));
            }
            fs::create_dir_all(parent).map_err(|err| HostError::HostCall(err.to_string()))?;
        }
        let canonical_parent = parent
            .canonicalize()
            .map_err(|err| HostError::InvalidPath(format!("{}: {err}", parsed.display)))?;
        ensure_under_root(&canonical_parent, &policy.root, &parsed.display)?;
        Ok(ResolvedPath {
            alias: parsed.alias,
            relative: parsed.relative,
            display: parsed.display,
            path: target,
            policy,
        })
    }

    fn parse_path(&self, input: Option<&str>) -> Result<ParsedPath> {
        let default_input;
        let input = match input {
            Some(input) => input,
            None => {
                default_input = self
                    .first_alias()
                    .map(|alias| format!("{alias}:"))
                    .ok_or_else(|| {
                        HostError::InvalidPath("no linked folders configured".to_string())
                    })?;
                default_input.as_str()
            }
        };
        parse_linked_path(input)
    }

    pub(super) fn read_resource(
        &self,
        path: &str,
        selector: Option<&str>,
    ) -> Result<ResourceContent> {
        let resolved = self.resolve_existing(Some(path))?;
        let bytes = fs::read(&resolved.path).map_err(|err| HostError::HostCall(err.to_string()))?;
        let content = String::from_utf8(bytes).map_err(|_| {
            HostError::InvalidArgs("fs.read supports UTF-8 text only in P0".to_string())
        })?;
        let size_bytes = content.len();
        let (selected, has_more) = select_text(&content, selector)?;
        Ok(ResourceContent {
            uri: linked_uri(&resolved.alias, &resolved.relative),
            kind: "text".to_string(),
            mime: "text/plain".to_string(),
            title: resolved
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            size_bytes,
            selector: selector.map(str::to_string),
            has_more,
            preview: preview(&selected, 1024),
            content: selected,
        })
    }
}

fn policy_from_config(config: LinkedFolderConfig) -> Result<FsPolicy> {
    let mut commands = BTreeSet::new();
    for command in config.commands {
        validate_command_name(&command)?;
        commands.insert(command);
    }
    let policy = FsPolicy {
        alias: config.name,
        root: config.path,
        mode: config.mode,
        commands,
        safe_args: config.safe_args,
    };
    validate_policy(policy)
}

fn validate_policy(mut policy: FsPolicy) -> Result<FsPolicy> {
    validate_alias(&policy.alias)?;
    let root = policy
        .root
        .canonicalize()
        .map_err(|err| HostError::InvalidPath(format!("{}: {err}", policy.root.display())))?;
    if !root.is_dir() {
        return Err(HostError::InvalidPath(format!(
            "linked folder {} is not a directory",
            root.display()
        )));
    }
    for command in &policy.commands {
        validate_command_name(command)?;
    }
    for argv in &policy.safe_args {
        let Some(command) = argv.first() else {
            return Err(HostError::InvalidArgs(
                "safe_args entries must be non-empty".into(),
            ));
        };
        if !policy.commands.contains(command) {
            return Err(HostError::InvalidArgs(format!(
                "safe_args command {command} is not in commands"
            )));
        }
    }
    policy.root = root;
    Ok(policy)
}

#[derive(Debug, Clone)]
pub(super) struct ParsedPath {
    pub(super) alias: String,
    pub(super) relative: PathBuf,
    pub(super) display: String,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedPath {
    pub(super) alias: String,
    pub(super) relative: PathBuf,
    pub(super) display: String,
    pub(super) path: PathBuf,
    pub(super) policy: FsPolicy,
}
