use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use tm_artifacts::{ResourceContent, preview};
use tokio::sync::{Mutex, MutexGuard};

use crate::{EgressConfig, HostError, Result, SelfEvolutionConfig};

use super::secure_fs::{FileIdentity, SecureKind, open_existing, pin_root_identity};
use super::util::{
    linked_uri, parse_linked_path, read_text_page_from_file, validate_alias, validate_command_name,
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
    #[serde(default = "default_proc_run_timeout_ms")]
    pub proc_run_timeout_ms: u64,
    #[serde(default)]
    pub self_evolution: SelfEvolutionConfig,
    #[serde(default)]
    pub egress: EgressConfig,
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
        if !(1..=900_000).contains(&self.proc_run_timeout_ms) {
            return Err(HostError::InvalidArgs(
                "proc_run_timeout_ms must be between 1 and 900000".to_string(),
            ));
        }
        self.egress.validate()?;
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

pub fn default_proc_run_timeout_ms() -> u64 {
    180_000
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsPolicy {
    pub alias: String,
    pub root: PathBuf,
    pub mode: FsMode,
    pub commands: BTreeSet<String>,
    pub safe_args: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct LinkedFolders {
    policies: Arc<RwLock<BTreeMap<String, RegisteredPolicy>>>,
    /// Orders policy replacement/removal against the final filesystem commit or process spawn.
    /// Readers hold this only for a bounded synchronous filesystem operation; approval waits never
    /// retain it.
    policy_gate: Arc<RwLock<()>>,
    mutations: Arc<Mutex<()>>,
    revision: Arc<AtomicU64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredPolicy {
    policy: FsPolicy,
    root_identity: FileIdentity,
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
            let root_identity = pin_root_identity(&policy.root, &policy.alias)?;
            policies.insert(
                policy.alias.clone(),
                RegisteredPolicy {
                    policy,
                    root_identity,
                },
            );
        }
        Ok(Self {
            policies: Arc::new(RwLock::new(policies)),
            policy_gate: Arc::new(RwLock::new(())),
            mutations: Arc::new(Mutex::new(())),
            revision: Arc::new(AtomicU64::new(1)),
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
            .map(|policies| {
                policies
                    .values()
                    .map(|registered| registered.policy.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) async fn lock_mutations(&self) -> MutexGuard<'_, ()> {
        self.mutations.lock().await
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(super) fn ensure_revision(&self, expected: u64) -> Result<()> {
        if self.revision() != expected {
            return Err(HostError::CapabilityDenied(
                "linked-folder policy changed while the operation was pending; retry".to_string(),
            ));
        }
        Ok(())
    }

    /// Runs one bounded read, final mutation commit, or process validation+spawn against a stable
    /// policy revision. `insert_policy` and `remove_policy` take the write side, so they cannot
    /// slip between the revision check/policy resolution performed by `operation` and its final
    /// syscall.
    pub(super) fn with_stable_policy_snapshot<T>(
        &self,
        expected_revision: u64,
        operation: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        let _gate = self.policy_gate.read().map_err(|err| {
            HostError::HostCall(format!("linked folder policy gate poisoned: {err}"))
        })?;
        self.ensure_revision(expected_revision)?;
        operation(self)
    }

    pub fn policy(&self, alias: &str) -> Result<FsPolicy> {
        self.policies
            .read()
            .map_err(|err| HostError::HostCall(format!("linked folder registry poisoned: {err}")))?
            .get(alias)
            .map(|registered| registered.policy.clone())
            .ok_or_else(|| HostError::InvalidPath(format!("unknown linked folder alias {alias}")))
    }

    fn registered_policy(&self, alias: &str) -> Result<RegisteredPolicy> {
        self.policies
            .read()
            .map_err(|err| HostError::HostCall(format!("linked folder registry poisoned: {err}")))?
            .get(alias)
            .cloned()
            .ok_or_else(|| HostError::InvalidPath(format!("unknown linked folder alias {alias}")))
    }

    pub fn insert_policy(&self, policy: FsPolicy) -> Result<FsPolicy> {
        let policy = validate_policy(policy)?;
        let root_identity = pin_root_identity(&policy.root, &policy.alias)?;
        let _gate = self.policy_gate.write().map_err(|err| {
            HostError::HostCall(format!("linked folder policy gate poisoned: {err}"))
        })?;
        let mut policies = self.policies.write().map_err(|err| {
            HostError::HostCall(format!("linked folder registry poisoned: {err}"))
        })?;
        if let Some(existing) = policies.get(&policy.alias) {
            if existing.policy == policy && existing.root_identity == root_identity {
                return Ok(existing.policy.clone());
            }
            if existing.policy.root == policy.root {
                policies.insert(
                    policy.alias.clone(),
                    RegisteredPolicy {
                        policy: policy.clone(),
                        root_identity,
                    },
                );
                self.revision.fetch_add(1, Ordering::AcqRel);
                return Ok(policy);
            }
            return Err(HostError::InvalidArgs(format!(
                "duplicate linked folder alias {}",
                policy.alias
            )));
        }
        policies.insert(
            policy.alias.clone(),
            RegisteredPolicy {
                policy: policy.clone(),
                root_identity,
            },
        );
        self.revision.fetch_add(1, Ordering::AcqRel);
        Ok(policy)
    }

    pub fn remove_policy(&self, alias: &str) -> Result<FsPolicy> {
        validate_alias(alias)?;
        let _gate = self.policy_gate.write().map_err(|err| {
            HostError::HostCall(format!("linked folder policy gate poisoned: {err}"))
        })?;
        let removed = self
            .policies
            .write()
            .map_err(|err| HostError::HostCall(format!("linked folder registry poisoned: {err}")))?
            .remove(alias)
            .ok_or_else(|| {
                HostError::InvalidPath(format!("unknown linked folder alias {alias}"))
            })?;
        self.revision.fetch_add(1, Ordering::AcqRel);
        Ok(removed.policy)
    }

    pub(super) fn resolve_spec(&self, input: Option<&str>) -> Result<ResolvedPath> {
        let parsed = self.parse_path(input)?;
        let registered = self.registered_policy(&parsed.alias)?;
        Ok(ResolvedPath {
            alias: parsed.alias,
            relative: parsed.relative.clone(),
            display: parsed.display,
            policy: registered.policy,
            root_identity: registered.root_identity,
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
        let revision = self.revision();
        self.with_stable_policy_snapshot(revision, |linked| {
            let resolved = linked.resolve_spec(Some(path))?;
            let handle = open_existing(
                &resolved.policy,
                resolved.root_identity,
                &resolved.relative,
                &resolved.display,
            )?;
            if handle.kind != SecureKind::File {
                return Err(HostError::InvalidPath(format!(
                    "{} is not a regular file",
                    resolved.display
                )));
            }
            let size_bytes = handle
                .size_bytes
                .ok_or_else(|| {
                    HostError::InvalidPath(format!("{} is not a file", resolved.display))
                })?
                .try_into()
                .map_err(|_| HostError::HostCall("linked file is too large".to_string()))?;
            let (selected, has_more) = read_text_page_from_file(handle.file, selector)?;
            Ok(ResourceContent {
                uri: linked_uri(&resolved.alias, &resolved.relative),
                kind: "text".to_string(),
                mime: "text/plain".to_string(),
                title: resolved
                    .relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string),
                size_bytes,
                selector: selector.map(str::to_string),
                has_more,
                preview: preview(&selected, 1024),
                content: selected,
            })
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
    pub(super) policy: FsPolicy,
    pub(super) root_identity: FileIdentity,
}
