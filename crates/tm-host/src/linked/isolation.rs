use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};
#[cfg(target_os = "linux")]
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};

use crate::{HostError, Result};

use super::FsMode;

#[cfg(target_os = "linux")]
mod cgroup;
#[cfg(any(target_os = "linux", test))]
mod seccomp;

const MIN_ADDRESS_SPACE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ADDRESS_SPACE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_PROCESS_COUNT: u64 = 512;
const MAX_OPEN_FILES: u64 = 4096;
const MAX_RUNTIME_ROOTS: usize = 16;
const MAX_CONFIG_PATH_BYTES: usize = 4096;
const PINNED_EXECUTABLE_PATH: &str = "/run/tempestmiku/proc-run-executable";

/// OS isolation applied around `proc.run` after the normal linked-folder, argv, and approval
/// checks. Disabled is deliberately the default: production deployments must opt into a complete
/// Linux profile, and an enabled profile never falls back to direct host execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcIsolationConfig {
    Disabled {},
    LinuxBubblewrap {
        launcher: PathBuf,
        runtime_roots: Vec<PathBuf>,
        #[serde(default)]
        limits: ProcIsolationLimits,
    },
    /// A fixed higher-assurance developer profile. Unlike `linux_bubblewrap`, this provider is
    /// unavailable unless both the repo-owned `developer_v1` seccomp policy and an already
    /// delegated cgroup-v2 subtree can be pinned before approval. It never falls back to the lower
    /// profile or direct host execution.
    LinuxHardenedV1 {
        launcher: PathBuf,
        runtime_roots: Vec<PathBuf>,
        #[serde(default)]
        limits: ProcIsolationLimits,
        cgroup_root: PathBuf,
        #[serde(default)]
        cgroup_limits: ProcCgroupV2Limits,
    },
}

impl Default for ProcIsolationConfig {
    fn default() -> Self {
        Self::Disabled {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcIsolationLimits {
    #[serde(default = "default_address_space_bytes")]
    pub address_space_bytes: u64,
    #[serde(default = "default_process_count")]
    pub process_count: u64,
    #[serde(default = "default_open_files")]
    pub open_files: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcCgroupV2Limits {
    #[serde(default = "default_cgroup_memory_max_bytes")]
    pub memory_max_bytes: u64,
    #[serde(default)]
    pub memory_swap_max_bytes: u64,
    #[serde(default = "default_cgroup_pids_max")]
    pub pids_max: u64,
    #[serde(default = "default_cgroup_cpu_quota_micros")]
    pub cpu_quota_micros: u64,
    #[serde(default = "default_cgroup_cpu_period_micros")]
    pub cpu_period_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcIsolationRecoveredLeaf {
    pub name: String,
    pub counters: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcIsolationRecoveryReport {
    pub provider: String,
    pub cgroup_root: PathBuf,
    pub recovered: Vec<ProcIsolationRecoveredLeaf>,
}

impl Default for ProcIsolationLimits {
    fn default() -> Self {
        Self {
            address_space_bytes: default_address_space_bytes(),
            process_count: default_process_count(),
            open_files: default_open_files(),
        }
    }
}

impl Default for ProcCgroupV2Limits {
    fn default() -> Self {
        Self {
            memory_max_bytes: default_cgroup_memory_max_bytes(),
            memory_swap_max_bytes: 0,
            pids_max: default_cgroup_pids_max(),
            cpu_quota_micros: default_cgroup_cpu_quota_micros(),
            cpu_period_micros: default_cgroup_cpu_period_micros(),
        }
    }
}

const fn default_address_space_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}

const fn default_process_count() -> u64 {
    128
}

const fn default_open_files() -> u64 {
    1024
}

const fn default_cgroup_memory_max_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}

const fn default_cgroup_pids_max() -> u64 {
    128
}

const fn default_cgroup_cpu_quota_micros() -> u64 {
    100_000
}

const fn default_cgroup_cpu_period_micros() -> u64 {
    100_000
}

impl ProcIsolationConfig {
    /// Whether this profile owns per-run kernel state that must be recovered before a service
    /// instance starts accepting work. The delegated subtree is exclusive to that instance; a
    /// supervisor must never point concurrently running instances at the same root.
    pub fn requires_startup_orphan_recovery(&self) -> bool {
        matches!(self, Self::LinuxHardenedV1 { .. })
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_for_os(std::env::consts::OS)
    }

    pub fn validate_runtime(&self) -> Result<()> {
        self.prepare().map(|_| ())
    }

    /// Kill and remove stale `tm-run-v1-*` leaves after a service crash. Call this only during
    /// exclusive startup, before accepting any new `proc.run`; the configured delegated subtree is
    /// intentionally treated as owned by this service instance.
    pub fn recover_orphans_at_startup(&self) -> Result<ProcIsolationRecoveryReport> {
        match self {
            Self::LinuxHardenedV1 { .. } => {
                self.validate()?;
                #[cfg(target_os = "linux")]
                {
                    let PreparedProcIsolation::LinuxBubblewrap(profile) = self.prepare()? else {
                        unreachable!("linux_hardened_v1 always prepares bubblewrap")
                    };
                    let BubblewrapAssurance::LinuxHardenedV1(hardened) = profile.assurance else {
                        unreachable!("linux_hardened_v1 always prepares hardened assurance")
                    };
                    hardened.cgroup.recover_orphans_at_startup()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Err(HostError::CapabilityDenied(
                        "linux_hardened_v1 recovery requires Linux".to_string(),
                    ))
                }
            }
            _ => Err(HostError::InvalidArgs(
                "orphan recovery is available only for linux_hardened_v1".to_string(),
            )),
        }
    }

    fn validate_for_os(&self, os: &str) -> Result<()> {
        let (launcher, runtime_roots, limits, hardened) = match self {
            Self::Disabled {} => return Ok(()),
            Self::LinuxBubblewrap {
                launcher,
                runtime_roots,
                limits,
            } => (launcher, runtime_roots, limits, None),
            Self::LinuxHardenedV1 {
                launcher,
                runtime_roots,
                limits,
                cgroup_root,
                cgroup_limits,
            } => (
                launcher,
                runtime_roots,
                limits,
                Some((cgroup_root, cgroup_limits)),
            ),
        };
        if os != "linux" {
            return Err(HostError::InvalidArgs(
                "configured Linux proc isolation provider requires Linux".to_string(),
            ));
        }
        validate_absolute_path(launcher, "proc isolation launcher")?;
        validate_path_size(launcher, "proc isolation launcher")?;
        if runtime_roots.is_empty() {
            return Err(HostError::InvalidArgs(
                "proc isolation runtime_roots must contain at least one explicit runtime path"
                    .to_string(),
            ));
        }
        if runtime_roots.len() > MAX_RUNTIME_ROOTS {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation runtime_roots cannot exceed {MAX_RUNTIME_ROOTS} entries"
            )));
        }
        let mut roots = BTreeSet::new();
        for root in runtime_roots {
            validate_absolute_path(root, "proc isolation runtime root")?;
            validate_path_size(root, "proc isolation runtime root")?;
            if is_forbidden_runtime_root(root) {
                return Err(HostError::InvalidArgs(format!(
                    "proc isolation runtime root {} is too broad or host-sensitive",
                    root.display()
                )));
            }
            if !roots.insert(root) {
                return Err(HostError::InvalidArgs(format!(
                    "duplicate proc isolation runtime root {}",
                    root.display()
                )));
            }
        }
        limits.validate()?;
        if let Some((cgroup_root, cgroup_limits)) = hardened {
            validate_absolute_path(cgroup_root, "proc isolation cgroup_root")?;
            validate_path_size(cgroup_root, "proc isolation cgroup_root")?;
            if cgroup_root == Path::new("/sys/fs/cgroup") {
                return Err(HostError::InvalidArgs(
                    "linux_hardened_v1 requires a dedicated delegated cgroup subtree, not the cgroup-v2 mount root"
                        .to_string(),
                ));
            }
            cgroup_limits.validate()?;
        }
        Ok(())
    }

    pub(crate) fn prepare(&self) -> Result<PreparedProcIsolation> {
        match self {
            Self::Disabled {} => Ok(PreparedProcIsolation::Disabled),
            Self::LinuxBubblewrap { .. } | Self::LinuxHardenedV1 { .. } => {
                self.validate()?;
                #[cfg(target_os = "linux")]
                {
                    self.prepare_linux_bubblewrap()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Err(HostError::CapabilityDenied(
                        "configured proc isolation is unavailable on this operating system"
                            .to_string(),
                    ))
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn prepare_linux_bubblewrap(&self) -> Result<PreparedProcIsolation> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let (launcher, configured_runtime_roots, limits) = match self {
            Self::LinuxBubblewrap {
                launcher,
                runtime_roots,
                limits,
            }
            | Self::LinuxHardenedV1 {
                launcher,
                runtime_roots,
                limits,
                ..
            } => (launcher, runtime_roots, limits),
            Self::Disabled {} => unreachable!("caller matched a Linux provider"),
        };
        let launcher = canonical_trusted_path(launcher, "proc isolation launcher", false)?;
        validate_trusted_ancestry(&launcher, "proc isolation launcher")?;
        let metadata = launcher.metadata().map_err(|error| {
            HostError::CapabilityDenied(format!(
                "proc isolation launcher {} is unavailable: {error}",
                launcher.display()
            ))
        })?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(HostError::CapabilityDenied(format!(
                "proc isolation launcher {} is not an executable regular file",
                launcher.display()
            )));
        }
        if metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0 {
            return Err(HostError::CapabilityDenied(format!(
                "proc isolation launcher {} must be root-owned and not group/world writable",
                launcher.display()
            )));
        }

        let mut canonical_roots = BTreeSet::new();
        let mut runtime_mounts = Vec::with_capacity(configured_runtime_roots.len());
        for root in configured_runtime_roots {
            let canonical = canonical_trusted_path(root, "proc isolation runtime root", true)?;
            validate_trusted_ancestry(&canonical, "proc isolation runtime root")?;
            if is_forbidden_runtime_root(&canonical) {
                return Err(HostError::CapabilityDenied(format!(
                    "proc isolation runtime root {} resolves to a broad or host-sensitive path",
                    canonical.display()
                )));
            }
            let metadata = canonical.metadata().map_err(|error| {
                HostError::CapabilityDenied(format!(
                    "proc isolation runtime root {} cannot be inspected: {error}",
                    canonical.display()
                ))
            })?;
            if metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0 {
                return Err(HostError::CapabilityDenied(format!(
                    "proc isolation runtime root {} must be root-owned and not group/world writable",
                    canonical.display()
                )));
            }
            canonical_roots.insert(canonical.clone());
            runtime_mounts.push(RuntimeMount {
                source: canonical,
                destination: root.clone(),
            });
        }
        runtime_mounts.sort_by(|left, right| left.destination.cmp(&right.destination));
        let runtime_roots = canonical_roots.into_iter().collect::<Vec<_>>();
        if !path_visible(&launcher, &runtime_roots, None) {
            return Err(HostError::CapabilityDenied(format!(
                "proc isolation launcher {} is not covered by runtime_roots",
                launcher.display()
            )));
        }

        let identity = (metadata.dev(), metadata.ino());
        let launcher_file = open_pinned_file(&launcher, identity, "proc isolation launcher")?;
        let assurance = match self {
            Self::LinuxBubblewrap { .. } => BubblewrapAssurance::NamespaceRlimits,
            Self::LinuxHardenedV1 {
                cgroup_root,
                cgroup_limits,
                ..
            } => {
                let seccomp = PreparedSeccomp::developer_v1()?;
                let cgroup = cgroup::PreparedCgroupRoot::prepare(cgroup_root, *cgroup_limits)?;
                BubblewrapAssurance::LinuxHardenedV1(PreparedLinuxHardened { seccomp, cgroup })
            }
            Self::Disabled {} => unreachable!("caller matched a Linux provider"),
        };
        let digest = isolation_digest(&launcher, identity, &runtime_mounts, *limits, &assurance)?;
        Ok(PreparedProcIsolation::LinuxBubblewrap(Box::new(
            PreparedBubblewrap {
                launcher,
                launcher_identity: identity,
                launcher_file: Some(Arc::new(launcher_file)),
                runtime_roots,
                runtime_mounts,
                limits: *limits,
                profile_sha256: digest,
                assurance,
            },
        )))
    }
}

impl ProcIsolationLimits {
    fn validate(self) -> Result<()> {
        if !(MIN_ADDRESS_SPACE_BYTES..=MAX_ADDRESS_SPACE_BYTES).contains(&self.address_space_bytes)
        {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation address_space_bytes must be between {MIN_ADDRESS_SPACE_BYTES} and {MAX_ADDRESS_SPACE_BYTES}"
            )));
        }
        if !(1..=MAX_PROCESS_COUNT).contains(&self.process_count) {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation process_count must be between 1 and {MAX_PROCESS_COUNT}"
            )));
        }
        if !(16..=MAX_OPEN_FILES).contains(&self.open_files) {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation open_files must be between 16 and {MAX_OPEN_FILES}"
            )));
        }
        Ok(())
    }
}

impl ProcCgroupV2Limits {
    fn validate(self) -> Result<()> {
        if !(MIN_ADDRESS_SPACE_BYTES..=MAX_ADDRESS_SPACE_BYTES).contains(&self.memory_max_bytes) {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation cgroup memory_max_bytes must be between {MIN_ADDRESS_SPACE_BYTES} and {MAX_ADDRESS_SPACE_BYTES}"
            )));
        }
        if self.memory_swap_max_bytes > MAX_ADDRESS_SPACE_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation cgroup memory_swap_max_bytes cannot exceed {MAX_ADDRESS_SPACE_BYTES}"
            )));
        }
        if !(1..=MAX_PROCESS_COUNT).contains(&self.pids_max) {
            return Err(HostError::InvalidArgs(format!(
                "proc isolation cgroup pids_max must be between 1 and {MAX_PROCESS_COUNT}"
            )));
        }
        if !(1_000..=1_000_000).contains(&self.cpu_period_micros) {
            return Err(HostError::InvalidArgs(
                "proc isolation cgroup cpu_period_micros must be between 1000 and 1000000"
                    .to_string(),
            ));
        }
        if !(1_000..=self.cpu_period_micros.saturating_mul(64)).contains(&self.cpu_quota_micros) {
            return Err(HostError::InvalidArgs(
                "proc isolation cgroup cpu_quota_micros must be between 1000 and 64 times cpu_period_micros"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreparedProcIsolation {
    Disabled,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    LinuxBubblewrap(Box<PreparedBubblewrap>),
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedBubblewrap {
    launcher: PathBuf,
    launcher_identity: (u64, u64),
    #[cfg(target_os = "linux")]
    launcher_file: Option<Arc<File>>,
    runtime_roots: Vec<PathBuf>,
    runtime_mounts: Vec<RuntimeMount>,
    limits: ProcIsolationLimits,
    profile_sha256: String,
    assurance: BubblewrapAssurance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RuntimeMount {
    source: PathBuf,
    destination: PathBuf,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum BubblewrapAssurance {
    NamespaceRlimits,
    #[cfg(target_os = "linux")]
    LinuxHardenedV1(PreparedLinuxHardened),
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedLinuxHardened {
    seccomp: PreparedSeccomp,
    cgroup: cgroup::PreparedCgroupRoot,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct PreparedSeccomp {
    version: &'static str,
    arch: seccomp::LinuxAuditArch,
    sha256: String,
    bytes: usize,
    file: Arc<File>,
}

#[cfg(target_os = "linux")]
impl PreparedSeccomp {
    fn developer_v1() -> Result<Self> {
        let arch = seccomp::LinuxAuditArch::current()?;
        let policy = seccomp::developer_v1_policy(arch);
        let sha256 = seccomp::policy_sha256(&policy);
        let bytes = policy.len();
        let file = seccomp::sealed_policy_file(&policy)?;
        Ok(Self {
            version: seccomp::POLICY_VERSION,
            arch,
            sha256,
            bytes,
            file: Arc::new(file),
        })
    }
}

#[cfg(target_os = "linux")]
impl PartialEq for PreparedSeccomp {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.arch == other.arch
            && self.sha256 == other.sha256
            && self.bytes == other.bytes
    }
}

#[cfg(target_os = "linux")]
impl Eq for PreparedSeccomp {}

impl PartialEq for PreparedBubblewrap {
    fn eq(&self, other: &Self) -> bool {
        self.launcher == other.launcher
            && self.launcher_identity == other.launcher_identity
            && self.runtime_roots == other.runtime_roots
            && self.runtime_mounts == other.runtime_mounts
            && self.limits == other.limits
            && self.profile_sha256 == other.profile_sha256
            && self.assurance == other.assurance
    }
}

impl Eq for PreparedBubblewrap {}

impl PreparedBubblewrap {
    fn provider_name(&self) -> &'static str {
        match &self.assurance {
            BubblewrapAssurance::NamespaceRlimits => "linux_bubblewrap",
            #[cfg(target_os = "linux")]
            BubblewrapAssurance::LinuxHardenedV1(_) => "linux_hardened_v1",
        }
    }

    fn hardening_approval_details(&self) -> Value {
        match &self.assurance {
            BubblewrapAssurance::NamespaceRlimits => json!({
                "level": "namespace_rlimits",
                "seccomp": false,
                "cgroupV2": false,
            }),
            #[cfg(target_os = "linux")]
            BubblewrapAssurance::LinuxHardenedV1(hardened) => json!({
                "level": "linux_hardened_v1",
                "seccomp": {
                    "version": hardened.seccomp.version,
                    "architecture": hardened.seccomp.arch.name(),
                    "policySha256": hardened.seccomp.sha256,
                    "policyBytes": hardened.seccomp.bytes,
                },
                "cgroupV2": hardened.cgroup.approval_details(),
            }),
        }
    }

    #[cfg(target_os = "linux")]
    fn seccomp_file(&self) -> Option<Arc<File>> {
        match &self.assurance {
            BubblewrapAssurance::NamespaceRlimits => None,
            BubblewrapAssurance::LinuxHardenedV1(hardened) => {
                Some(Arc::clone(&hardened.seccomp.file))
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn seccomp_fd(&self) -> Option<libc::c_int> {
        self.seccomp_file().map(|file| file.as_raw_fd())
    }
}

pub(crate) enum ProcIsolationRun {
    None,
    #[cfg(target_os = "linux")]
    LinuxHardenedV1(cgroup::CgroupRun),
}

impl ProcIsolationRun {
    #[cfg(target_os = "linux")]
    pub(crate) fn cgroup_procs_file(&self) -> Option<Arc<File>> {
        match self {
            Self::None => None,
            Self::LinuxHardenedV1(run) => Some(run.cgroup_procs_file()),
        }
    }

    pub(crate) fn terminate_and_cleanup(&mut self) -> Result<()> {
        match self {
            Self::None => Ok(()),
            #[cfg(target_os = "linux")]
            Self::LinuxHardenedV1(run) => run.terminate_and_cleanup().map(|_| ()),
        }
    }
}

pub(crate) struct ProcIsolationCommand<'a> {
    pub resolved_executable: &'a Path,
    pub executable_target: &'a Path,
    pub command_name: &'a str,
    pub args: &'a [String],
    pub linked_root: &'a Path,
    /// Descriptor opened by the descriptor-relative linked-folder resolver. Bubblewrap consumes
    /// this with `--bind-fd`, so a later replacement of `linked_root` cannot change the mount.
    pub linked_root_fd: libc::c_int,
    pub linked_mode: FsMode,
    pub cwd: &'a Path,
    /// Independently pinned cwd descriptor. It is over-mounted at `cwd` after the root mount so a
    /// replacement of the cwd entry inside an otherwise stable linked root cannot redirect chdir.
    pub cwd_fd: libc::c_int,
    /// Executable descriptor pinned before approval. Bubblewrap mounts this exact inode at a
    /// private path instead of reopening the PATH target after approval.
    pub executable_fd: libc::c_int,
    pub environment: &'a [(String, OsString)],
}

impl PreparedProcIsolation {
    pub(crate) fn approval_details(&self) -> Value {
        match self {
            Self::Disabled => json!({
                "provider": "disabled",
                "networkIsolated": false,
            }),
            Self::LinuxBubblewrap(profile) => json!({
                "provider": profile.provider_name(),
                "networkIsolated": true,
                "profileSha256": profile.profile_sha256,
                "launcher": profile.launcher,
                "launcherDevice": profile.launcher_identity.0,
                "launcherInode": profile.launcher_identity.1,
                "addressSpaceBytes": profile.limits.address_space_bytes,
                "processCount": profile.limits.process_count,
                "openFiles": profile.limits.open_files,
                "hardening": profile.hardening_approval_details(),
            }),
        }
    }

    pub(crate) fn start_run(&self) -> Result<ProcIsolationRun> {
        match self {
            Self::Disabled => Ok(ProcIsolationRun::None),
            Self::LinuxBubblewrap(profile) => match &profile.assurance {
                BubblewrapAssurance::NamespaceRlimits => Ok(ProcIsolationRun::None),
                #[cfg(target_os = "linux")]
                BubblewrapAssurance::LinuxHardenedV1(hardened) => hardened
                    .cgroup
                    .start_run()
                    .map(ProcIsolationRun::LinuxHardenedV1),
            },
        }
    }

    pub(crate) fn command(
        &self,
        spec: ProcIsolationCommand<'_>,
    ) -> Result<tokio::process::Command> {
        let Self::LinuxBubblewrap(profile) = self else {
            return Err(HostError::HostCall(
                "disabled isolation does not construct a launcher command".to_string(),
            ));
        };
        if !path_visible(
            spec.executable_target,
            &profile.runtime_roots,
            Some(spec.linked_root),
        ) || !path_visible(
            spec.resolved_executable,
            &profile.runtime_roots,
            Some(spec.linked_root),
        ) {
            return Err(HostError::CapabilityDenied(format!(
                "proc.run executable {} is not covered by the isolation runtime roots or linked folder",
                spec.resolved_executable.display()
            )));
        }
        if !spec.cwd.starts_with(spec.linked_root) {
            return Err(HostError::CapabilityDenied(
                "proc isolation cwd escaped the linked-folder mount".to_string(),
            ));
        }

        #[cfg(target_os = "linux")]
        let mut command = {
            let launcher_file = profile.launcher_file.clone().ok_or_else(|| {
                HostError::CapabilityDenied(
                    "proc isolation launcher descriptor is unavailable".to_string(),
                )
            })?;
            let launcher_fd = launcher_file.as_raw_fd();
            let seccomp_file = profile.seccomp_file();
            let mut command = tokio::process::Command::new(format!("/proc/self/fd/{launcher_fd}"));
            let expected_launcher_identity = profile.launcher_identity;
            // The launcher itself is descriptor-pinned. The closure keeps the descriptor alive
            // through spawn, verifies its identity in the child, and makes it inheritable for
            // procfs execution.
            unsafe {
                command.pre_exec(move || {
                    verify_fd_identity(launcher_file.as_raw_fd(), expected_launcher_identity)?;
                    inherit_fd(launcher_file.as_raw_fd())?;
                    if let Some(seccomp_file) = &seccomp_file {
                        inherit_fd(seccomp_file.as_raw_fd())?;
                    }
                    Ok(())
                });
            }
            command
        };
        #[cfg(not(target_os = "linux"))]
        let mut command = tokio::process::Command::new(&profile.launcher);
        command
            .arg("--die-with-parent")
            .arg("--new-session")
            .arg("--unshare-all")
            // `--unshare-all` intentionally does not imply a user namespace in bubblewrap.
            // Create it explicitly before locking out nested user namespaces.
            .arg("--unshare-user")
            .arg("--disable-userns")
            .arg("--cap-drop")
            .arg("ALL")
            .arg("--clearenv")
            .arg("--proc")
            .arg("/proc")
            .arg("--dev")
            .arg("/dev")
            .arg("--tmpfs")
            .arg("/tmp")
            .arg("--dir")
            .arg("/run")
            .arg("--dir")
            .arg("/run/tempestmiku");
        #[cfg(target_os = "linux")]
        if let Some(seccomp_fd) = profile.seccomp_fd() {
            command.arg("--add-seccomp-fd").arg(seccomp_fd.to_string());
        }
        for mount in &profile.runtime_mounts {
            command
                .arg("--ro-bind")
                .arg(&mount.source)
                .arg(&mount.destination);
        }
        command
            .arg(match spec.linked_mode {
                FsMode::Ro => "--ro-bind-fd",
                FsMode::Rw => "--bind-fd",
            })
            .arg(spec.linked_root_fd.to_string())
            .arg(spec.linked_root)
            // The root fd pins the tree, while this second bind pins the exact directory approved
            // as cwd. The destination is only a name in the newly built mount namespace.
            .arg(match spec.linked_mode {
                FsMode::Ro => "--ro-bind-fd",
                FsMode::Rw => "--bind-fd",
            })
            .arg(spec.cwd_fd.to_string())
            .arg(spec.cwd)
            .arg("--ro-bind-fd")
            .arg(spec.executable_fd.to_string())
            .arg(PINNED_EXECUTABLE_PATH)
            .arg("--chdir")
            .arg(spec.cwd);
        for (key, value) in
            isolated_environment(spec.environment, &profile.runtime_roots, spec.linked_root)?
        {
            command.arg("--setenv").arg(key).arg(value);
        }
        command
            .arg("--argv0")
            .arg(spec.command_name)
            .arg("--")
            .arg(PINNED_EXECUTABLE_PATH)
            .args(spec.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        Ok(command)
    }

    #[cfg(unix)]
    pub(crate) fn prepare_child(
        &self,
        executable_fd: Option<libc::c_int>,
        pinned_mount_fds: Option<(libc::c_int, libc::c_int)>,
        cgroup_procs_fd: Option<libc::c_int>,
    ) -> std::io::Result<()> {
        let Self::LinuxBubblewrap(profile) = self else {
            return if pinned_mount_fds.is_none() && cgroup_procs_fd.is_none() {
                if let Some(executable_fd) = executable_fd {
                    inherit_fd(executable_fd)
                } else {
                    Ok(())
                }
            } else {
                Err(std::io::Error::from_raw_os_error(libc::EINVAL))
            };
        };
        let Some(executable_fd) = executable_fd else {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        };
        let Some((linked_root_fd, cwd_fd)) = pinned_mount_fds else {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        };
        #[cfg(target_os = "linux")]
        if let Some(cgroup_procs_fd) = cgroup_procs_fd {
            cgroup::join_current_process(cgroup_procs_fd)?;
        } else if matches!(profile.assurance, BubblewrapAssurance::LinuxHardenedV1(_)) {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        }
        #[cfg(not(target_os = "linux"))]
        if cgroup_procs_fd.is_some() {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        }
        inherit_fd(executable_fd)?;
        inherit_fd(linked_root_fd)?;
        inherit_fd(cwd_fd)?;
        set_limit(
            libc::RLIMIT_AS as libc::c_int,
            profile.limits.address_space_bytes,
        )?;
        set_limit(
            libc::RLIMIT_NPROC as libc::c_int,
            profile.limits.process_count,
        )?;
        set_limit(
            libc::RLIMIT_NOFILE as libc::c_int,
            profile.limits.open_files,
        )
    }
}

#[cfg(unix)]
fn inherit_fd(fd: libc::c_int) -> std::io::Result<()> {
    // `open_existing` deliberately returns CLOEXEC descriptors. Clear that bit only in the forked
    // child so bubblewrap can consume the descriptor with `--bind-fd`; the parent remains CLOEXEC
    // and concurrent proc.run calls cannot inherit one another's pins.
    // SAFETY: `fd` is a live descriptor owned by a `File` captured in the pre-exec closure.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: F_SETFD only updates descriptor flags for this child-side descriptor table.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn validate_absolute_path(path: &Path, label: &str) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(HostError::InvalidArgs(format!(
            "{label} must be an absolute normalized path"
        )));
    }
    Ok(())
}

fn validate_path_size(path: &Path, label: &str) -> Result<()> {
    if path.as_os_str().to_string_lossy().len() > MAX_CONFIG_PATH_BYTES {
        return Err(HostError::InvalidArgs(format!(
            "{label} cannot exceed {MAX_CONFIG_PATH_BYTES} bytes"
        )));
    }
    Ok(())
}

fn is_forbidden_runtime_root(path: &Path) -> bool {
    const FORBIDDEN: &[&str] = &[
        "/", "/boot", "/dev", "/etc", "/home", "/proc", "/root", "/run", "/sys", "/tmp", "/var",
    ];
    FORBIDDEN
        .iter()
        .any(|forbidden| path == Path::new(forbidden))
}

#[cfg(target_os = "linux")]
fn canonical_trusted_path(path: &Path, label: &str, directory: bool) -> Result<PathBuf> {
    let canonical = path.canonicalize().map_err(|error| {
        HostError::CapabilityDenied(format!(
            "{label} {} is unavailable: {error}",
            path.display()
        ))
    })?;
    let metadata = canonical.metadata().map_err(|error| {
        HostError::CapabilityDenied(format!(
            "{label} {} cannot be inspected: {error}",
            canonical.display()
        ))
    })?;
    if directory && !metadata.is_dir() {
        return Err(HostError::CapabilityDenied(format!(
            "{label} {} is not a directory",
            canonical.display()
        )));
    }
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn validate_trusted_ancestry(path: &Path, label: &str) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    for ancestor in path.ancestors().skip(1) {
        let metadata = ancestor.metadata().map_err(|error| {
            HostError::CapabilityDenied(format!(
                "{label} ancestor {} cannot be inspected: {error}",
                ancestor.display()
            ))
        })?;
        let mode = metadata.permissions().mode();
        let writable = mode & 0o022 != 0;
        // A root-owned sticky directory (normally /tmp) does not let an unprivileged peer rename
        // or replace this root-owned child. Other group/world-writable ancestors make the
        // configured path replaceable and are rejected.
        let sticky_root_directory = metadata.uid() == 0 && mode & libc::S_ISVTX != 0;
        if !metadata.is_dir() || metadata.uid() != 0 || (writable && !sticky_root_directory) {
            return Err(HostError::CapabilityDenied(format!(
                "{label} ancestor {} must be root-owned and not group/world writable (except a root-owned sticky directory)",
                ancestor.display()
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_pinned_file(path: &Path, expected: (u64, u64), label: &str) -> Result<File> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

    let path_bytes = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| HostError::HostCall(format!("{label} path contains a NUL byte")))?;
    // O_PATH pins executable identity without requiring read permission.
    // SAFETY: `path_bytes` is NUL-terminated and these flags require no mode argument.
    let fd = unsafe { libc::open(path_bytes.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(HostError::CapabilityDenied(format!(
            "{label} {} cannot be pinned: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `open` returned a new owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|error| {
        HostError::CapabilityDenied(format!(
            "{label} {} pinned descriptor cannot be inspected: {error}",
            path.display()
        ))
    })?;
    if (metadata.dev(), metadata.ino()) != expected {
        return Err(HostError::CapabilityDenied(format!(
            "{label} {} changed while it was being pinned; retry",
            path.display()
        )));
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn verify_fd_identity(fd: libc::c_int, expected: (u64, u64)) -> std::io::Result<()> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd` is live and `stat` points to writable storage for one result.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized the value.
    let stat = unsafe { stat.assume_init() };
    if (stat.st_dev, stat.st_ino) != expected {
        return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
    }
    Ok(())
}

fn path_visible(path: &Path, runtime_roots: &[PathBuf], linked_root: Option<&Path>) -> bool {
    runtime_roots.iter().any(|root| path.starts_with(root))
        || linked_root.is_some_and(|root| path.starts_with(root))
}

fn isolated_environment(
    environment: &[(String, OsString)],
    runtime_roots: &[PathBuf],
    linked_root: &Path,
) -> Result<Vec<(String, OsString)>> {
    environment
        .iter()
        .map(|(key, value)| {
            if key != "PATH" {
                return Ok((key.clone(), value.clone()));
            }
            let visible = std::env::split_paths(value)
                .filter(|path| path_visible(path, runtime_roots, Some(linked_root)))
                .collect::<Vec<_>>();
            if visible.is_empty() {
                return Err(HostError::CapabilityDenied(
                    "proc isolation removed every PATH entry; runtime_roots do not cover an executable search path"
                        .to_string(),
                ));
            }
            std::env::join_paths(visible)
                .map(|path| (key.clone(), path))
                .map_err(|error| HostError::HostCall(error.to_string()))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn isolation_digest(
    launcher: &Path,
    launcher_identity: (u64, u64),
    runtime_mounts: &[RuntimeMount],
    limits: ProcIsolationLimits,
    assurance: &BubblewrapAssurance,
) -> Result<String> {
    let hardening = match assurance {
        BubblewrapAssurance::NamespaceRlimits => json!({
            "level": "namespace_rlimits",
        }),
        BubblewrapAssurance::LinuxHardenedV1(hardened) => json!({
            "level": "linux_hardened_v1",
            "seccomp": {
                "version": hardened.seccomp.version,
                "architecture": hardened.seccomp.arch.name(),
                "sha256": hardened.seccomp.sha256,
                "bytes": hardened.seccomp.bytes,
            },
            "cgroupV2": hardened.cgroup.digest_details(),
        }),
    };
    let encoded = serde_json::to_vec(&json!({
        "provider": match assurance {
            BubblewrapAssurance::NamespaceRlimits => "linux_bubblewrap",
            BubblewrapAssurance::LinuxHardenedV1(_) => "linux_hardened_v1",
        },
        "launcher": launcher,
        "launcherDevice": launcher_identity.0,
        "launcherInode": launcher_identity.1,
        "runtimeRoots": runtime_mounts,
        "network": "unshared",
        "limits": limits,
        "hardening": hardening,
    }))
    .map_err(|error| HostError::HostCall(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

#[cfg(unix)]
fn set_limit(resource: libc::c_int, value: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` points to a fully initialized rlimit value for this child process.
    if unsafe { libc::setrlimit(resource as _, &limit) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn linux_profile() -> ProcIsolationConfig {
        ProcIsolationConfig::LinuxBubblewrap {
            launcher: PathBuf::from("/usr/bin/bwrap"),
            runtime_roots: vec![PathBuf::from("/usr"), PathBuf::from("/nix/store")],
            limits: ProcIsolationLimits::default(),
        }
    }

    fn prepared_plan() -> PreparedProcIsolation {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;

            let launcher = std::env::current_exe().unwrap();
            let metadata = launcher.metadata().unwrap();
            let identity = (metadata.dev(), metadata.ino());
            let launcher_file = open_pinned_file(&launcher, identity, "test launcher").unwrap();
            return PreparedProcIsolation::LinuxBubblewrap(Box::new(PreparedBubblewrap {
                launcher,
                launcher_identity: identity,
                launcher_file: Some(Arc::new(launcher_file)),
                runtime_roots: vec![PathBuf::from("/usr"), PathBuf::from("/usr/lib")],
                runtime_mounts: vec![
                    RuntimeMount {
                        source: PathBuf::from("/usr/lib"),
                        destination: PathBuf::from("/lib"),
                    },
                    RuntimeMount {
                        source: PathBuf::from("/usr"),
                        destination: PathBuf::from("/usr"),
                    },
                ],
                limits: ProcIsolationLimits::default(),
                profile_sha256: "digest".to_string(),
                assurance: BubblewrapAssurance::NamespaceRlimits,
            }));
        }
        #[cfg(not(target_os = "linux"))]
        PreparedProcIsolation::LinuxBubblewrap(Box::new(PreparedBubblewrap {
            launcher: PathBuf::from("/usr/bin/bwrap"),
            launcher_identity: (1, 2),
            runtime_roots: vec![PathBuf::from("/usr"), PathBuf::from("/usr/lib")],
            runtime_mounts: vec![
                RuntimeMount {
                    source: PathBuf::from("/usr/lib"),
                    destination: PathBuf::from("/lib"),
                },
                RuntimeMount {
                    source: PathBuf::from("/usr"),
                    destination: PathBuf::from("/usr"),
                },
            ],
            limits: ProcIsolationLimits::default(),
            profile_sha256: "digest".to_string(),
            assurance: BubblewrapAssurance::NamespaceRlimits,
        }))
    }

    #[test]
    fn isolation_defaults_to_disabled_and_enabled_profile_is_linux_only() {
        assert_eq!(
            ProcIsolationConfig::default(),
            ProcIsolationConfig::Disabled {}
        );
        assert!(!ProcIsolationConfig::default().requires_startup_orphan_recovery());
        assert!(
            serde_json::from_value::<ProcIsolationConfig>(json!({
                "provider": "disabled",
                "share_net": true,
            }))
            .is_err()
        );
        assert!(
            ProcIsolationConfig::default()
                .validate_for_os("macos")
                .is_ok()
        );
        assert!(!linux_profile().requires_startup_orphan_recovery());
        let error = linux_profile().validate_for_os("macos").unwrap_err();
        assert!(error.to_string().contains("requires Linux"));
        linux_profile().validate_for_os("linux").unwrap();
        let hardened = ProcIsolationConfig::LinuxHardenedV1 {
            launcher: PathBuf::from("/usr/bin/bwrap"),
            runtime_roots: vec![PathBuf::from("/usr")],
            limits: ProcIsolationLimits::default(),
            cgroup_root: PathBuf::from("/sys/fs/cgroup/tempestmiku.service"),
            cgroup_limits: ProcCgroupV2Limits::default(),
        };
        assert!(hardened.requires_startup_orphan_recovery());
        assert!(hardened.validate_for_os("linux").is_ok());
        assert!(hardened.validate_for_os("macos").is_err());
    }

    #[test]
    fn isolation_rejects_ambient_or_malformed_mount_profiles() {
        for root in ["/", "/home", "/etc", "/proc", "/tmp"] {
            let profile = ProcIsolationConfig::LinuxBubblewrap {
                launcher: PathBuf::from("/usr/bin/bwrap"),
                runtime_roots: vec![PathBuf::from(root)],
                limits: ProcIsolationLimits::default(),
            };
            assert!(profile.validate_for_os("linux").is_err(), "accepted {root}");
        }
        let relative = ProcIsolationConfig::LinuxBubblewrap {
            launcher: PathBuf::from("bwrap"),
            runtime_roots: vec![PathBuf::from("/usr")],
            limits: ProcIsolationLimits::default(),
        };
        assert!(relative.validate_for_os("linux").is_err());
        let duplicate = ProcIsolationConfig::LinuxBubblewrap {
            launcher: PathBuf::from("/usr/bin/bwrap"),
            runtime_roots: vec![PathBuf::from("/usr"), PathBuf::from("/usr")],
            limits: ProcIsolationLimits::default(),
        };
        assert!(duplicate.validate_for_os("linux").is_err());
        let too_many = ProcIsolationConfig::LinuxBubblewrap {
            launcher: PathBuf::from("/usr/bin/bwrap"),
            runtime_roots: (0..=MAX_RUNTIME_ROOTS)
                .map(|index| PathBuf::from(format!("/opt/runtime-{index}")))
                .collect(),
            limits: ProcIsolationLimits::default(),
        };
        assert!(too_many.validate_for_os("linux").is_err());
    }

    #[test]
    fn isolation_bounds_resource_limits() {
        let mut profile = linux_profile();
        if let ProcIsolationConfig::LinuxBubblewrap { limits, .. } = &mut profile {
            limits.process_count = 0;
        }
        assert!(profile.validate_for_os("linux").is_err());
        if let ProcIsolationConfig::LinuxBubblewrap { limits, .. } = &mut profile {
            limits.process_count = 1;
            limits.open_files = 15;
        }
        assert!(profile.validate_for_os("linux").is_err());
        if let ProcIsolationConfig::LinuxBubblewrap { limits, .. } = &mut profile {
            limits.open_files = 16;
            limits.address_space_bytes = MIN_ADDRESS_SPACE_BYTES - 1;
        }
        assert!(profile.validate_for_os("linux").is_err());
    }

    #[test]
    fn hardened_profile_rejects_mount_root_and_invalid_cgroup_limits() {
        let mut profile = ProcIsolationConfig::LinuxHardenedV1 {
            launcher: PathBuf::from("/usr/bin/bwrap"),
            runtime_roots: vec![PathBuf::from("/usr")],
            limits: ProcIsolationLimits::default(),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_limits: ProcCgroupV2Limits::default(),
        };
        assert!(profile.validate_for_os("linux").is_err());
        if let ProcIsolationConfig::LinuxHardenedV1 {
            cgroup_root,
            cgroup_limits,
            ..
        } = &mut profile
        {
            *cgroup_root = PathBuf::from("/sys/fs/cgroup/tempestmiku.service");
            cgroup_limits.pids_max = 0;
        }
        assert!(profile.validate_for_os("linux").is_err());
        if let ProcIsolationConfig::LinuxHardenedV1 { cgroup_limits, .. } = &mut profile {
            cgroup_limits.pids_max = 1;
            cgroup_limits.cpu_quota_micros = 999;
        }
        assert!(profile.validate_for_os("linux").is_err());
    }

    #[test]
    fn bubblewrap_plan_is_argv_only_and_never_shares_network_or_host_root() {
        let prepared = prepared_plan();
        let path = OsString::from("/usr/bin");
        let command = prepared
            .command(ProcIsolationCommand {
                resolved_executable: Path::new("/usr/bin/cargo"),
                executable_target: Path::new("/usr/bin/cargo"),
                command_name: "cargo",
                args: &["test".to_string()],
                linked_root: Path::new("/workspace/repo"),
                linked_root_fd: 11,
                linked_mode: FsMode::Rw,
                cwd: Path::new("/workspace/repo/crate"),
                cwd_fd: 12,
                executable_fd: 13,
                environment: &[("PATH".to_string(), path)],
            })
            .unwrap();
        let argv = command
            .as_std()
            .get_args()
            .map(OsStr::to_string_lossy)
            .collect::<Vec<_>>();
        assert!(argv.iter().any(|arg| arg == "--unshare-all"));
        assert!(argv.iter().any(|arg| arg == "--unshare-user"));
        assert!(argv.iter().any(|arg| arg == "--disable-userns"));
        assert!(
            argv.windows(2)
                .any(|window| { window[0] == "--cap-drop" && window[1] == "ALL" })
        );
        assert!(argv.iter().any(|arg| arg == "--clearenv"));
        assert!(argv.windows(3).any(|window| {
            window[0] == "--ro-bind" && window[1] == "/usr/lib" && window[2] == "/lib"
        }));
        assert!(!argv.iter().any(|arg| arg == "--share-net"));
        assert!(
            !argv
                .windows(3)
                .any(|window| { window[0] == "--ro-bind" && window[1] == "/" && window[2] == "/" })
        );
        assert!(argv.windows(3).any(|window| {
            window[0] == "--bind-fd" && window[1] == "11" && window[2] == "/workspace/repo"
        }));
        assert!(argv.windows(3).any(|window| {
            window[0] == "--bind-fd" && window[1] == "12" && window[2] == "/workspace/repo/crate"
        }));
        assert!(argv.windows(3).any(|window| {
            window[0] == "--ro-bind-fd" && window[1] == "13" && window[2] == PINNED_EXECUTABLE_PATH
        }));
        assert!(!argv.windows(3).any(|window| {
            window[0] == "--bind"
                && window[1] == "/workspace/repo"
                && window[2] == "/workspace/repo"
        }));
        assert!(argv.ends_with(&["--".into(), PINNED_EXECUTABLE_PATH.into(), "test".into()]));
    }

    #[test]
    fn bubblewrap_plan_fails_when_executable_or_cwd_is_outside_mounts() {
        let prepared = prepared_plan();
        let env = [("PATH".to_string(), OsString::from("/usr/bin"))];
        let error = prepared
            .command(ProcIsolationCommand {
                resolved_executable: Path::new("/opt/bin/tool"),
                executable_target: Path::new("/opt/bin/tool"),
                command_name: "tool",
                args: &[],
                linked_root: Path::new("/workspace/repo"),
                linked_root_fd: 11,
                linked_mode: FsMode::Ro,
                cwd: Path::new("/workspace/repo"),
                cwd_fd: 12,
                executable_fd: 13,
                environment: &env,
            })
            .unwrap_err();
        assert!(matches!(error, HostError::CapabilityDenied(_)));
        let error = prepared
            .command(ProcIsolationCommand {
                resolved_executable: Path::new("/usr/bin/tool"),
                executable_target: Path::new("/usr/bin/tool"),
                command_name: "tool",
                args: &[],
                linked_root: Path::new("/workspace/repo"),
                linked_root_fd: 11,
                linked_mode: FsMode::Ro,
                cwd: Path::new("/outside"),
                cwd_fd: 12,
                executable_fd: 13,
                environment: &env,
            })
            .unwrap_err();
        assert!(matches!(error, HostError::CapabilityDenied(_)));
    }
}
