use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{HostError, Result};

use super::{ProcCgroupV2Limits, ProcIsolationRecoveredLeaf, ProcIsolationRecoveryReport};

const CGROUP2_SUPER_MAGIC: libc::c_long = 0x6367_7270;
const RUN_PREFIX: &str = "tm-run-v1-";
const PROBE_PREFIX: &str = "tm-probe-v1-";
const MAX_CONTROL_BYTES: usize = 64 * 1024;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub(super) struct PreparedCgroupRoot {
    path: PathBuf,
    identity: (u64, u64),
    limits: ProcCgroupV2Limits,
    root: Arc<File>,
}

impl PartialEq for PreparedCgroupRoot {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.identity == other.identity && self.limits == other.limits
    }
}

impl Eq for PreparedCgroupRoot {}

impl PreparedCgroupRoot {
    pub(super) fn prepare(path: &Path, limits: ProcCgroupV2Limits) -> Result<Self> {
        let canonical = path.canonicalize().map_err(|error| {
            denied(format!(
                "delegated cgroup root {} is unavailable: {error}",
                path.display()
            ))
        })?;
        let metadata = canonical.metadata().map_err(|error| {
            denied(format!(
                "delegated cgroup root {} cannot be inspected: {error}",
                canonical.display()
            ))
        })?;
        if !metadata.is_dir() {
            return Err(denied(format!(
                "delegated cgroup root {} is not a directory",
                canonical.display()
            )));
        }
        let identity = (metadata.dev(), metadata.ino());
        let root = Arc::new(open_directory(&canonical)?);
        verify_directory_identity(&root, identity, "delegated cgroup root")?;
        verify_cgroup2(&root)?;
        verify_delegation(&root)?;

        let prepared = Self {
            path: canonical,
            identity,
            limits,
            root,
        };
        // A real create/configure/remove probe is the only reliable way to prove the delegated
        // root is writable for every controller. It happens before approval and leaves no process
        // in the probe leaf.
        prepared.probe_writable()?;
        Ok(prepared)
    }

    pub(super) fn approval_details(&self) -> Value {
        json!({
            "root": self.path,
            "rootDevice": self.identity.0,
            "rootInode": self.identity.1,
            "memoryMaxBytes": self.limits.memory_max_bytes,
            "memorySwapMaxBytes": self.limits.memory_swap_max_bytes,
            "pidsMax": self.limits.pids_max,
            "cpuMax": format!("{} {}", self.limits.cpu_quota_micros, self.limits.cpu_period_micros),
        })
    }

    pub(super) fn digest_details(&self) -> Value {
        self.approval_details()
    }

    pub(super) fn start_run(&self) -> Result<CgroupRun> {
        self.verify_current()?;
        let (name, leaf) = self.create_unique_leaf(RUN_PREFIX)?;
        if let Err(error) = configure_leaf(&leaf, self.limits) {
            let _ = kill_leaf(&leaf);
            let _ = remove_leaf(&self.root, &name);
            return Err(error);
        }
        let cgroup_procs = match open_control(&leaf, c"cgroup.procs", libc::O_WRONLY) {
            Ok(file) => Arc::new(file),
            Err(error) => {
                let _ = kill_leaf(&leaf);
                let _ = remove_leaf(&self.root, &name);
                return Err(error);
            }
        };
        Ok(CgroupRun {
            root: self.clone(),
            name,
            leaf: Arc::new(leaf),
            cgroup_procs,
            cleaned: false,
        })
    }

    pub(super) fn recover_orphans_at_startup(&self) -> Result<ProcIsolationRecoveryReport> {
        self.verify_current()?;
        let mut names = std::fs::read_dir(&self.path)
            .map_err(|error| {
                denied(format!(
                    "delegated cgroup root {} cannot be enumerated: {error}",
                    self.path.display()
                ))
            })?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| valid_run_leaf_name(name))
            .collect::<Vec<_>>();
        names.sort();
        self.verify_current()?;

        let mut recovered = Vec::with_capacity(names.len());
        for name in names {
            let leaf = open_child_directory(&self.root, &name)?;
            let counters = terminate_leaf(&leaf)?;
            remove_leaf(&self.root, &name)?;
            recovered.push(ProcIsolationRecoveredLeaf { name, counters });
        }
        Ok(ProcIsolationRecoveryReport {
            provider: "linux_hardened_v1".to_string(),
            cgroup_root: self.path.clone(),
            recovered,
        })
    }

    fn verify_current(&self) -> Result<()> {
        verify_directory_identity(&self.root, self.identity, "delegated cgroup root")?;
        let current = self.path.metadata().map_err(|error| {
            denied(format!(
                "delegated cgroup root {} cannot be re-inspected: {error}",
                self.path.display()
            ))
        })?;
        if (current.dev(), current.ino()) != self.identity {
            return Err(denied(
                "delegated cgroup root changed after the isolation profile was prepared",
            ));
        }
        verify_delegation(&self.root)
    }

    fn probe_writable(&self) -> Result<()> {
        let (name, leaf) = self.create_unique_leaf(PROBE_PREFIX)?;
        let result = configure_leaf(&leaf, self.limits).and_then(|()| {
            // Opening both files proves join and kill authority without moving the service process
            // or signalling anything.
            open_control(&leaf, c"cgroup.procs", libc::O_WRONLY)?;
            open_control(&leaf, c"cgroup.kill", libc::O_WRONLY)?;
            Ok(())
        });
        let cleanup = remove_leaf(&self.root, &name);
        result.and(cleanup)
    }

    fn create_unique_leaf(&self, prefix: &str) -> Result<(String, File)> {
        for _ in 0..8 {
            let name = format!("{prefix}{}", Uuid::new_v4().simple());
            let c_name = CString::new(name.as_bytes()).expect("UUID leaf name has no NUL");
            // SAFETY: root is a live directory fd and c_name is a single relative component.
            let result = unsafe { libc::mkdirat(self.root.as_raw_fd(), c_name.as_ptr(), 0o700) };
            if result == 0 {
                return match open_child_directory(&self.root, &name) {
                    Ok(leaf) => Ok((name, leaf)),
                    Err(error) => {
                        let _ = remove_leaf(&self.root, &name);
                        Err(error)
                    }
                };
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(denied(format!(
                    "delegated cgroup root {} cannot create an execution leaf: {error}",
                    self.path.display()
                )));
            }
        }
        Err(denied(
            "delegated cgroup root repeatedly collided on unpredictable execution leaf names",
        ))
    }
}

#[derive(Debug)]
pub(crate) struct CgroupRun {
    root: PreparedCgroupRoot,
    name: String,
    leaf: Arc<File>,
    cgroup_procs: Arc<File>,
    cleaned: bool,
}

impl CgroupRun {
    pub(super) fn cgroup_procs_file(&self) -> Arc<File> {
        Arc::clone(&self.cgroup_procs)
    }

    pub(super) fn terminate_and_cleanup(&mut self) -> Result<BTreeMap<String, u64>> {
        if self.cleaned {
            return Ok(BTreeMap::new());
        }
        let counters = terminate_leaf(&self.leaf)?;
        remove_leaf(&self.root.root, &self.name)?;
        self.cleaned = true;
        Ok(counters)
    }
}

impl Drop for CgroupRun {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = terminate_leaf(&self.leaf);
            let _ = remove_leaf(&self.root.root, &self.name);
            self.cleaned = true;
        }
    }
}

pub(super) fn join_current_process(cgroup_procs_fd: RawFd) -> std::io::Result<()> {
    let bytes = b"0\n";
    // SAFETY: the fd is an open cgroup.procs descriptor captured by the parent. Writing `0` moves
    // the calling pre-exec child into that cgroup and does not allocate after fork.
    let written = unsafe {
        libc::write(
            cgroup_procs_fd,
            bytes.as_ptr().cast::<libc::c_void>(),
            bytes.len(),
        )
    };
    if written == bytes.len() as isize {
        Ok(())
    } else if written < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            "short write to cgroup.procs",
        ))
    }
}

fn configure_leaf(leaf: &File, limits: ProcCgroupV2Limits) -> Result<()> {
    write_control(
        leaf,
        c"memory.max",
        &format!("{}\n", limits.memory_max_bytes),
    )?;
    write_control(
        leaf,
        c"memory.swap.max",
        &format!("{}\n", limits.memory_swap_max_bytes),
    )?;
    write_control(leaf, c"pids.max", &format!("{}\n", limits.pids_max))?;
    write_control(
        leaf,
        c"cpu.max",
        &format!("{} {}\n", limits.cpu_quota_micros, limits.cpu_period_micros),
    )?;
    // Read back exact normalized values so a controller that silently clamps or ignores a setting
    // cannot pass the software gate.
    expect_control(leaf, c"memory.max", &limits.memory_max_bytes.to_string())?;
    expect_control(
        leaf,
        c"memory.swap.max",
        &limits.memory_swap_max_bytes.to_string(),
    )?;
    expect_control(leaf, c"pids.max", &limits.pids_max.to_string())?;
    expect_control(
        leaf,
        c"cpu.max",
        &format!("{} {}", limits.cpu_quota_micros, limits.cpu_period_micros),
    )?;
    Ok(())
}

fn expect_control(directory: &File, name: &CStr, expected: &str) -> Result<()> {
    let actual = read_control(directory, name)?;
    if actual.trim() == expected {
        Ok(())
    } else {
        Err(denied(format!(
            "cgroup controller {} read back {:?}, expected {:?}",
            name.to_string_lossy(),
            actual.trim(),
            expected
        )))
    }
}

fn terminate_leaf(leaf: &File) -> Result<BTreeMap<String, u64>> {
    kill_leaf(leaf)?;
    wait_unpopulated(leaf)?;
    read_counters(leaf)
}

fn kill_leaf(leaf: &File) -> Result<()> {
    write_control(leaf, c"cgroup.kill", "1\n")
}

fn wait_unpopulated(leaf: &File) -> Result<()> {
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        let events = parse_key_values(&read_control(leaf, c"cgroup.events")?)?;
        if events.get("populated") == Some(&0) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(HostError::HostCall(
                "linux_hardened_v1 cgroup remained populated after cgroup.kill".to_string(),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn read_counters(leaf: &File) -> Result<BTreeMap<String, u64>> {
    let mut counters = BTreeMap::new();
    for (file_name, prefix) in [
        (c"memory.events", "memory"),
        (c"pids.events", "pids"),
        (c"cpu.stat", "cpu"),
    ] {
        for (name, value) in parse_key_values(&read_control(leaf, file_name)?)? {
            counters.insert(format!("{prefix}.{name}"), value);
        }
    }
    Ok(counters)
}

fn parse_key_values(value: &str) -> Result<BTreeMap<String, u64>> {
    value
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let key = fields
                .next()
                .ok_or_else(|| HostError::HostCall("empty cgroup counter line".to_string()))?;
            let value = fields
                .next()
                .ok_or_else(|| HostError::HostCall(format!("cgroup counter {key} has no value")))?
                .parse::<u64>()
                .map_err(|error| {
                    HostError::HostCall(format!("invalid cgroup counter {key}: {error}"))
                })?;
            if fields.next().is_some() {
                return Err(HostError::HostCall(format!(
                    "cgroup counter {key} has unexpected fields"
                )));
            }
            Ok((key.to_string(), value))
        })
        .collect()
}

fn verify_delegation(root: &File) -> Result<()> {
    let controller_value = read_control(root, c"cgroup.controllers")?;
    let subtree_value = read_control(root, c"cgroup.subtree_control")?;
    let controllers = split_words(&controller_value);
    let subtree = split_words(&subtree_value);
    let required = BTreeSet::from(["cpu", "memory", "pids"]);
    let missing_controllers = required
        .difference(&controllers)
        .copied()
        .collect::<Vec<_>>();
    if !missing_controllers.is_empty() {
        return Err(denied(format!(
            "delegated cgroup root is missing controllers: {}",
            missing_controllers.join(", ")
        )));
    }
    let missing_subtree = required.difference(&subtree).copied().collect::<Vec<_>>();
    if !missing_subtree.is_empty() {
        return Err(denied(format!(
            "delegated cgroup root has not enabled child controllers: {}",
            missing_subtree.join(", ")
        )));
    }
    // A standard systemd delegation gives the service control of cgroup.procs and
    // cgroup.subtree_control at the delegated root, but intentionally keeps that root's
    // cgroup.kill owned by the manager. We never kill the delegated root. probe_writable()
    // creates a child leaf and proves cgroup.kill authority at the boundary we actually use.
    open_control(root, c"cgroup.procs", libc::O_WRONLY)?;
    Ok(())
}

fn split_words(value: &str) -> BTreeSet<&str> {
    value.split_whitespace().collect()
}

fn verify_cgroup2(root: &File) -> Result<()> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: root is live and stat points to storage for one statfs result.
    if unsafe { libc::fstatfs(root.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(denied(format!(
            "delegated cgroup root filesystem cannot be inspected: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: successful fstatfs initialized stat.
    let stat = unsafe { stat.assume_init() };
    if stat.f_type != CGROUP2_SUPER_MAGIC {
        return Err(denied(
            "linux_hardened_v1 cgroup_root is not on a cgroup-v2 filesystem",
        ));
    }
    Ok(())
}

fn verify_directory_identity(file: &File, expected: (u64, u64), label: &str) -> Result<()> {
    let metadata = file.metadata().map_err(|error| {
        denied(format!(
            "{label} pinned descriptor cannot be inspected: {error}"
        ))
    })?;
    if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != expected {
        return Err(denied(format!("{label} changed while it was being pinned")));
    }
    Ok(())
}

fn open_directory(path: &Path) -> Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| denied("delegated cgroup root path contains a NUL byte"))?;
    // SAFETY: path is a live C string and flags require no mode argument.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd(fd, "delegated cgroup root cannot be pinned")
}

fn open_child_directory(root: &File, name: &str) -> Result<File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| denied("cgroup execution leaf name contains a NUL byte"))?;
    // SAFETY: root is live and name is a single relative component.
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd(fd, "cgroup execution leaf cannot be opened")
}

fn open_control(directory: &File, name: &CStr, flags: libc::c_int) -> Result<File> {
    // SAFETY: directory is a live dirfd, name is a static single component, and no mode is needed.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd(
        fd,
        &format!(
            "required cgroup control {} is unavailable",
            name.to_string_lossy()
        ),
    )
}

fn owned_fd(fd: libc::c_int, context: &str) -> Result<File> {
    if fd < 0 {
        return Err(denied(format!(
            "{context}: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: the successful open returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_control(directory: &File, name: &CStr) -> Result<String> {
    let mut file = open_control(directory, name, libc::O_RDONLY)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_CONTROL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            denied(format!(
                "cgroup control {} cannot be read: {error}",
                name.to_string_lossy()
            ))
        })?;
    if bytes.len() > MAX_CONTROL_BYTES {
        return Err(denied(format!(
            "cgroup control {} exceeded {MAX_CONTROL_BYTES} bytes",
            name.to_string_lossy()
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        denied(format!(
            "cgroup control {} was not UTF-8: {error}",
            name.to_string_lossy()
        ))
    })
}

fn write_control(directory: &File, name: &CStr, value: &str) -> Result<()> {
    let mut file = open_control(directory, name, libc::O_WRONLY)?;
    file.write_all(value.as_bytes()).map_err(|error| {
        denied(format!(
            "cgroup control {} cannot be written: {error}",
            name.to_string_lossy()
        ))
    })
}

fn remove_leaf(root: &File, name: &str) -> Result<()> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| denied("cgroup execution leaf name contains a NUL byte"))?;
    // SAFETY: root is live, name is a single relative component, and AT_REMOVEDIR refuses files.
    let result = unsafe { libc::unlinkat(root.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if result == 0 {
        Ok(())
    } else {
        Err(HostError::HostCall(format!(
            "cgroup execution leaf {name:?} cannot be removed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

fn valid_run_leaf_name(name: &str) -> bool {
    let Some(uuid) = name.strip_prefix(RUN_PREFIX) else {
        return false;
    };
    uuid.len() == 32 && uuid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn denied(message: impl Into<String>) -> HostError {
    HostError::CapabilityDenied(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_only_claims_exact_unpredictable_run_leaf_names() {
        assert!(valid_run_leaf_name(
            "tm-run-v1-0123456789abcdef0123456789abcdef"
        ));
        for invalid in [
            "tm-run-v1-0123",
            "tm-run-v1-0123456789abcdef0123456789abcdeg",
            "tm-probe-v1-0123456789abcdef0123456789abcdef",
            "operator-owned",
            "../tm-run-v1-0123456789abcdef0123456789abcdef",
        ] {
            assert!(!valid_run_leaf_name(invalid), "accepted {invalid}");
        }
    }

    #[test]
    fn cgroup_counter_parser_is_strict_and_deterministic() {
        assert_eq!(
            parse_key_values("oom 2\noom_kill 1\n").unwrap(),
            BTreeMap::from([("oom".to_string(), 2), ("oom_kill".to_string(), 1)])
        );
        assert!(parse_key_values("oom\n").is_err());
        assert!(parse_key_values("oom nope\n").is_err());
        assert!(parse_key_values("oom 1 extra\n").is_err());
    }
}
