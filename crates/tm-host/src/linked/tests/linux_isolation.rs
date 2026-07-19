#![cfg(target_os = "linux")]

use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::Command as StdCommand;

use super::support::*;
use super::*;

const LINUX_ISOLATION_GATE: &str = "TM_LINUX_ISOLATION_TESTS";
const LINUX_HARDENED_GATE: &str = "TM_LINUX_HARDENED_TESTS";
const DEFAULT_BWRAP: &str = "/opt/tempestmiku-isolation-runtime/bin/bwrap";
const DEFAULT_RUNTIME_ROOT: &str = "/opt/tempestmiku-isolation-runtime";

struct PathGuard(Option<std::ffi::OsString>);

impl PathGuard {
    fn install(path: std::ffi::OsString) -> Self {
        let previous = std::env::var_os("PATH");
        // SAFETY: the gated canary is documented and invoked with `--test-threads=1`; no other
        // thread in that process reads or mutates PATH while the guard is active.
        unsafe { std::env::set_var("PATH", path) };
        Self(previous)
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        // SAFETY: see `install`; the same single test owns this environment mutation.
        unsafe {
            if let Some(previous) = self.0.take() {
                std::env::set_var("PATH", previous);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
}

fn gated_linux_profile() -> Option<ProcIsolationConfig> {
    if std::env::var(LINUX_ISOLATION_GATE).ok().as_deref() != Some("1") {
        return None;
    }
    let launcher = std::env::var_os("TM_LINUX_ISOLATION_BWRAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BWRAP));
    let runtime_root = std::env::var_os("TM_LINUX_ISOLATION_RUNTIME_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RUNTIME_ROOT));
    Some(ProcIsolationConfig::LinuxBubblewrap {
        launcher,
        runtime_roots: vec![runtime_root],
        limits: ProcIsolationLimits {
            address_space_bytes: 512 * 1024 * 1024,
            process_count: 16,
            open_files: 64,
        },
    })
}

fn gated_linux_hardened_profile() -> Option<ProcIsolationConfig> {
    if std::env::var(LINUX_HARDENED_GATE).ok().as_deref() != Some("1") {
        return None;
    }
    if let Some(host_config_path) = std::env::var_os("TM_HOST_CONFIG") {
        let host_config_path = PathBuf::from(host_config_path);
        let host_config = P0HostConfig::from_json_file(&host_config_path)
            .unwrap_or_else(|error| panic!("final TM_HOST_CONFIG failed Rust validation: {error}"));
        assert!(
            matches!(
                &host_config.proc_isolation,
                ProcIsolationConfig::LinuxHardenedV1 { .. }
            ),
            "final TM_HOST_CONFIG must select linux_hardened_v1"
        );
        return Some(host_config.proc_isolation);
    }
    let launcher = std::env::var_os("TM_LINUX_ISOLATION_BWRAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BWRAP));
    let runtime_roots = std::env::var_os("TM_LINUX_HARDENED_RUNTIME_ROOTS")
        .map(|roots| std::env::split_paths(&roots).collect::<Vec<_>>())
        .unwrap_or_else(|| vec![PathBuf::from(DEFAULT_RUNTIME_ROOT)]);
    let cgroup_root = std::env::var_os("TM_LINUX_HARDENED_CGROUP_ROOT")
        .map(PathBuf::from)
        .expect("TM_LINUX_HARDENED_CGROUP_ROOT is required for the gated hardened canary");
    Some(ProcIsolationConfig::LinuxHardenedV1 {
        launcher,
        runtime_roots,
        limits: ProcIsolationLimits {
            address_space_bytes: hardened_limit(
                "TM_LINUX_HARDENED_ADDRESS_SPACE_BYTES",
                2 * 1024 * 1024 * 1024,
                false,
            ),
            process_count: hardened_limit("TM_LINUX_HARDENED_PROCESS_COUNT", 64, false),
            open_files: hardened_limit("TM_LINUX_HARDENED_OPEN_FILES", 256, false),
        },
        cgroup_root,
        cgroup_limits: ProcCgroupV2Limits {
            memory_max_bytes: hardened_limit(
                "TM_LINUX_HARDENED_MEMORY_MAX_BYTES",
                1024 * 1024 * 1024,
                false,
            ),
            memory_swap_max_bytes: hardened_limit(
                "TM_LINUX_HARDENED_MEMORY_SWAP_MAX_BYTES",
                0,
                true,
            ),
            pids_max: hardened_limit("TM_LINUX_HARDENED_PIDS_MAX", 64, false),
            cpu_quota_micros: hardened_limit("TM_LINUX_HARDENED_CPU_QUOTA_MICROS", 100_000, false),
            cpu_period_micros: hardened_limit(
                "TM_LINUX_HARDENED_CPU_PERIOD_MICROS",
                100_000,
                false,
            ),
        },
    })
}

fn hardened_limit(name: &str, default: u64, allow_zero: bool) -> u64 {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    assert!(
        !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()),
        "{name} must be an unsigned decimal integer"
    );
    let value = raw
        .parse::<u64>()
        .unwrap_or_else(|error| panic!("{name} is outside the u64 range: {error}"));
    assert!(allow_zero || value > 0, "{name} must be greater than zero");
    value
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn delayed_launcher_profile(
    base: &ProcIsolationConfig,
    control_root: &Path,
) -> ProcIsolationConfig {
    let ProcIsolationConfig::LinuxBubblewrap {
        launcher,
        runtime_roots,
        limits,
    } = base
    else {
        unreachable!("the gated profile is always linux_bubblewrap")
    };
    assert_eq!(
        // SAFETY: this read-only libc query has no preconditions.
        unsafe { libc::geteuid() },
        0,
        "the gated replacement canary must run as root so its launcher is trusted"
    );
    let wrapper_root = control_root.join("wrapper-runtime");
    let wrapper_bin = wrapper_root.join("bin");
    fs::create_dir_all(&wrapper_bin).unwrap();
    let wrapper = wrapper_bin.join("bwrap");
    let ready = control_root.join("launcher-ready");
    let go = control_root.join("launcher-go");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nset -eu\n: > {}\nwhile [ ! -e {} ]; do /bin/sleep 0.01; done\nexec {} \"$@\"\n",
            shell_quote(&ready),
            shell_quote(&go),
            shell_quote(launcher),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut pinned_runtime_roots = vec![wrapper_root];
    pinned_runtime_roots.extend(runtime_roots.iter().cloned());
    ProcIsolationConfig::LinuxBubblewrap {
        launcher: wrapper,
        runtime_roots: pinned_runtime_roots,
        limits: *limits,
    }
}

async fn wait_for_launcher(control_root: &Path) {
    let ready = control_root.join("launcher-ready");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("delayed bubblewrap launcher did not start");
}

fn release_launcher(control_root: &Path) {
    fs::write(control_root.join("launcher-go"), b"go").unwrap();
}

fn isolated_proc_run(
    root: &Path,
    artifact_root: &Path,
    isolation: ProcIsolationConfig,
) -> ProcRunFn {
    ProcRunFn::with_timeout_and_isolation(
        temp_linked_with_commands(
            root,
            FsMode::Rw,
            [
                "cargo",
                "cat",
                "env",
                "mount",
                "race-exec",
                "resource-probe",
                "sh",
                "sleep",
                "test",
                "thread-probe",
                "touch",
                "unshare",
                "wget",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            Vec::new(),
        ),
        ArtifactStore::open(artifact_root, "linux-isolation-canary").unwrap(),
        60_000,
        isolation,
    )
}

fn hardened_leaf_names(cgroup_root: &Path) -> Vec<String> {
    let mut names = fs::read_dir(cgroup_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("tm-run-v1-"))
        .collect::<Vec<_>>();
    names.sort();
    names
}

async fn wait_for_path(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()));
}

/// Higher-assurance canary. The caller must provide an empty delegated cgroup-v2 subtree with
/// cpu/memory/pids enabled. It proves the fixed seccomp policy and per-run cgroup lifecycle through
/// the real proc.run path. Normal tests skip this body.
#[tokio::test]
async fn gated_linux_hardened_v1_enforces_seccomp_and_cgroup_lifecycle() {
    let Some(isolation) = gated_linux_hardened_profile() else {
        return;
    };
    let (cgroup_root, cgroup_limits, proc_limits) = match &isolation {
        ProcIsolationConfig::LinuxHardenedV1 {
            cgroup_root,
            cgroup_limits,
            limits,
            ..
        } => (cgroup_root.clone(), *cgroup_limits, *limits),
        _ => unreachable!("gate always builds linux_hardened_v1"),
    };
    isolation.validate_runtime().unwrap();
    assert!(hardened_leaf_names(&cgroup_root).is_empty());

    let runtime_root = match &isolation {
        ProcIsolationConfig::LinuxHardenedV1 { runtime_roots, .. } => {
            runtime_roots.first().unwrap()
        }
        _ => unreachable!("gate always builds linux_hardened_v1"),
    };
    assert!(
        runtime_root.join("bin/resource-probe").exists(),
        "static production-sizing resource probe is missing"
    );

    let root = tempfile::tempdir().unwrap();
    let artifact_root = tempfile::tempdir().unwrap();
    let proc_run = Arc::new(isolated_proc_run(
        root.path(),
        artifact_root.path(),
        isolation.clone(),
    ));

    let thread_probe = run_command(&proc_run, "thread-probe", Vec::new()).await;
    assert_eq!(
        thread_probe["stdout"],
        json!("thread-ok\nchild-ok\n"),
        "thread/fork probe: {thread_probe}"
    );
    let limits = run_command(&proc_run, "cat", vec!["/proc/self/limits".to_string()]).await;
    assert_eq!(limits["exitCode"], json!(0), "limits result: {limits}");
    let limit_output = limits["stdout"].as_str().unwrap();
    assert_proc_limit(
        limit_output,
        &["Max", "address", "space"],
        proc_limits.address_space_bytes,
    );
    assert_proc_limit(
        limit_output,
        &["Max", "processes"],
        proc_limits.process_count,
    );
    assert_proc_limit(
        limit_output,
        &["Max", "open", "files"],
        proc_limits.open_files,
    );

    let memory_ready = root.path().join("resource-memory-ready");
    let memory_run = {
        let proc_run = Arc::clone(&proc_run);
        tokio::spawn(async move {
            run_command(
                &proc_run,
                "resource-probe",
                vec![
                    "memory".to_string(),
                    (256_u64 * 1024 * 1024).to_string(),
                    "resource-memory-ready".to_string(),
                    "750".to_string(),
                ],
            )
            .await
        })
    };
    wait_for_path(&memory_ready).await;
    let leaves = hardened_leaf_names(&cgroup_root);
    assert_eq!(leaves.len(), 1, "memory probe leaves: {leaves:?}");
    let memory_current = fs::read_to_string(cgroup_root.join(&leaves[0]).join("memory.current"))
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    assert!(
        memory_current >= 256_u64 * 1024 * 1024,
        "memory probe current bytes: {memory_current}"
    );
    assert_eq!(memory_run.await.unwrap()["exitCode"], json!(0));
    assert!(hardened_leaf_names(&cgroup_root).is_empty());

    let pids_ready = root.path().join("resource-pids-ready");
    let pids_run = {
        let proc_run = Arc::clone(&proc_run);
        tokio::spawn(async move {
            run_command(
                &proc_run,
                "resource-probe",
                vec![
                    "pids".to_string(),
                    "16".to_string(),
                    "resource-pids-ready".to_string(),
                    "750".to_string(),
                ],
            )
            .await
        })
    };
    wait_for_path(&pids_ready).await;
    let leaves = hardened_leaf_names(&cgroup_root);
    assert_eq!(leaves.len(), 1, "pids probe leaves: {leaves:?}");
    let pids_current = fs::read_to_string(cgroup_root.join(&leaves[0]).join("pids.current"))
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap();
    assert!(
        pids_current >= 17,
        "pids probe current count: {pids_current}"
    );
    assert_eq!(pids_run.await.unwrap()["exitCode"], json!(0));
    assert!(hardened_leaf_names(&cgroup_root).is_empty());

    let cpu_ready = root.path().join("resource-cpu-ready");
    let cpu_run = {
        let proc_run = Arc::clone(&proc_run);
        tokio::spawn(async move {
            run_command(
                &proc_run,
                "resource-probe",
                vec![
                    "cpu".to_string(),
                    "750".to_string(),
                    "resource-cpu-ready".to_string(),
                ],
            )
            .await
        })
    };
    wait_for_path(&cpu_ready).await;
    let leaves = hardened_leaf_names(&cgroup_root);
    assert_eq!(leaves.len(), 1, "cpu probe leaves: {leaves:?}");
    let cpu_leaf = cgroup_root.join(&leaves[0]);
    let cpu_before = fs::read_to_string(cpu_leaf.join("cpu.stat")).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let cpu_after = fs::read_to_string(cpu_leaf.join("cpu.stat")).unwrap();
    assert_ne!(
        cpu_before, cpu_after,
        "cpu probe did not consume accounted CPU"
    );
    assert_eq!(cpu_run.await.unwrap()["exitCode"], json!(0));
    assert!(hardened_leaf_names(&cgroup_root).is_empty());
    for (command, args) in [
        ("unshare", vec!["--mount".to_string(), "true".to_string()]),
        (
            "mount",
            vec![
                "-t".to_string(),
                "tmpfs".to_string(),
                "none".to_string(),
                "/tmp".to_string(),
            ],
        ),
    ] {
        let denied = run_command(&proc_run, command, args).await;
        assert_ne!(denied["exitCode"], json!(0), "seccomp probe: {denied}");
    }

    let ready = root.path().join("ready");
    let running = {
        let proc_run = Arc::clone(&proc_run);
        tokio::spawn(async move {
            run_command(
                &proc_run,
                "sh",
                vec!["-c".to_string(), "touch ready; sleep 2".to_string()],
            )
            .await
        })
    };
    wait_for_path(&ready).await;
    let leaves = hardened_leaf_names(&cgroup_root);
    assert_eq!(leaves.len(), 1, "active leaves: {leaves:?}");
    let active_leaf = cgroup_root.join(&leaves[0]);
    assert_eq!(
        fs::read_to_string(active_leaf.join("memory.max"))
            .unwrap()
            .trim(),
        cgroup_limits.memory_max_bytes.to_string()
    );
    assert_eq!(
        fs::read_to_string(active_leaf.join("memory.swap.max"))
            .unwrap()
            .trim(),
        cgroup_limits.memory_swap_max_bytes.to_string()
    );
    assert_eq!(
        fs::read_to_string(active_leaf.join("pids.max"))
            .unwrap()
            .trim(),
        cgroup_limits.pids_max.to_string()
    );
    assert_eq!(
        fs::read_to_string(active_leaf.join("cpu.max"))
            .unwrap()
            .trim(),
        format!(
            "{} {}",
            cgroup_limits.cpu_quota_micros, cgroup_limits.cpu_period_micros
        )
    );
    assert_eq!(running.await.unwrap()["exitCode"], json!(0));
    assert!(hardened_leaf_names(&cgroup_root).is_empty());

    let timeout = proc_run
        .call(
            json!({
                "cmd": "sleep",
                "args": ["20"],
                "cwd": "tempestmiku:",
                "timeoutMs": 100,
            }),
            &approved_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(timeout["timedOut"], json!(true), "timeout: {timeout}");
    assert!(hardened_leaf_names(&cgroup_root).is_empty());

    let cancel_ready = root.path().join("cancel-ready");
    let cancelling = {
        let proc_run = Arc::clone(&proc_run);
        tokio::spawn(async move {
            run_command(
                &proc_run,
                "sh",
                vec!["-c".to_string(), "touch cancel-ready; sleep 20".to_string()],
            )
            .await
        })
    };
    wait_for_path(&cancel_ready).await;
    cancelling.abort();
    assert!(cancelling.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(hardened_leaf_names(&cgroup_root).is_empty());

    let orphan_name = "tm-run-v1-00000000000000000000000000000001";
    let orphan = cgroup_root.join(orphan_name);
    fs::create_dir(&orphan).unwrap();
    fs::write(
        orphan.join("memory.max"),
        format!("{}\n", cgroup_limits.memory_max_bytes),
    )
    .unwrap();
    fs::write(
        orphan.join("memory.swap.max"),
        format!("{}\n", cgroup_limits.memory_swap_max_bytes),
    )
    .unwrap();
    fs::write(
        orphan.join("pids.max"),
        format!("{}\n", cgroup_limits.pids_max),
    )
    .unwrap();
    fs::write(
        orphan.join("cpu.max"),
        format!(
            "{} {}\n",
            cgroup_limits.cpu_quota_micros, cgroup_limits.cpu_period_micros
        ),
    )
    .unwrap();
    let mut orphan_child = StdCommand::new("sleep").arg("20").spawn().unwrap();
    fs::write(
        orphan.join("cgroup.procs"),
        format!("{}\n", orphan_child.id()),
    )
    .unwrap();
    let recovery = isolation.recover_orphans_at_startup().unwrap();
    assert_eq!(recovery.recovered.len(), 1, "recovery: {recovery:?}");
    assert_eq!(recovery.recovered[0].name, orphan_name);
    assert!(hardened_leaf_names(&cgroup_root).is_empty());
    let status = orphan_child.wait().unwrap();
    assert!(!status.success(), "orphan process survived cgroup.kill");

    let capture = Arc::new(CaptureThenApprove::default());
    let captured_ctx =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    let captured = proc_run
        .call(
            json!({
                "cmd": "thread-probe",
                "cwd": "tempestmiku:",
            }),
            &captured_ctx,
        )
        .await
        .unwrap();
    assert_eq!(captured["exitCode"], json!(0));
    let actions = capture.actions.lock().unwrap();
    let action: Value = serde_json::from_str(&actions[0]).unwrap();
    let details = &action["details"]["isolation"];
    assert_eq!(details["provider"], json!("linux_hardened_v1"));
    assert_eq!(
        details["hardening"]["seccomp"]["version"],
        json!("developer_v1")
    );
    assert_eq!(details["hardening"]["cgroupV2"]["root"], json!(cgroup_root));
    assert_eq!(
        details["hardening"]["cgroupV2"]["pidsMax"],
        json!(cgroup_limits.pids_max)
    );
    assert_eq!(
        details["hardening"]["seccomp"]["policySha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );

    let unavailable_capture = Arc::new(CaptureThenApprove::default());
    let unavailable_ctx = InvocationCtx::with_approvals(
        ctx().grants,
        unavailable_capture.clone(),
        Duration::from_secs(1),
    );
    let mut unavailable = isolation;
    let ProcIsolationConfig::LinuxHardenedV1 { cgroup_root, .. } = &mut unavailable else {
        unreachable!()
    };
    *cgroup_root = PathBuf::from("/sys/fs/cgroup/definitely-missing-tempestmiku");
    let unavailable_proc = isolated_proc_run(root.path(), artifact_root.path(), unavailable);
    let error = unavailable_proc
        .call(
            json!({
                "cmd": "touch",
                "args": ["hardening-fallback-marker"],
                "cwd": "tempestmiku:",
            }),
            &unavailable_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::CapabilityDenied(_)));
    assert!(unavailable_capture.actions.lock().unwrap().is_empty());
    assert!(!root.path().join("hardening-fallback-marker").exists());
}

async fn run_command(proc_run: &ProcRunFn, command: &str, args: Vec<String>) -> Value {
    run_command_in(proc_run, command, args, "tempestmiku:").await
}

fn assert_proc_limit(output: &str, prefix: &[&str], expected: u64) {
    let fields = output
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| fields.starts_with(prefix))
        .unwrap_or_else(|| panic!("/proc/self/limits omitted {}", prefix.join(" ")));
    let expected = expected.to_string();
    assert_eq!(fields.get(prefix.len()), Some(&expected.as_str()));
    assert_eq!(fields.get(prefix.len() + 1), Some(&expected.as_str()));
}

async fn run_command_in(
    proc_run: &ProcRunFn,
    command: &str,
    args: Vec<String>,
    cwd: &str,
) -> Value {
    proc_run
        .call(
            json!({
                "cmd": command,
                "args": args,
                "cwd": cwd,
                "timeoutMs": 5_000,
            }),
            &approved_ctx(),
        )
        .await
        .unwrap()
}

/// This test is intentionally gated because it needs a real Linux kernel, a root-owned bubblewrap
/// launcher, and a root-owned static runtime. The default canary image copies bubblewrap and
/// installs BusyBox and direct applet symlinks under `/opt/tempestmiku-isolation-runtime/bin`, then
/// launches the test with that directory first in PATH. Normal `cargo test` runs remain
/// network-free and do not require bubblewrap.
#[tokio::test]
async fn gated_linux_bubblewrap_proc_run_enforces_profile() {
    let Some(isolation) = gated_linux_profile() else {
        return;
    };
    let runtime_root = match &isolation {
        ProcIsolationConfig::LinuxBubblewrap { runtime_roots, .. } => {
            runtime_roots.first().unwrap().clone()
        }
        ProcIsolationConfig::Disabled {} | ProcIsolationConfig::LinuxHardenedV1 { .. } => {
            unreachable!("gate always builds the low-assurance Linux profile")
        }
    };
    let runtime_bin = runtime_root.join("bin");
    assert!(
        runtime_bin.join("busybox").exists(),
        "static canary busybox is missing"
    );
    for command in ["cat", "env", "test", "touch", "wget"] {
        assert!(
            runtime_bin.join(command).exists(),
            "static canary {command} is missing"
        );
    }

    let root = tempfile::tempdir().unwrap();
    let artifact_root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("seed.txt"), "linked-folder-visible\n").unwrap();
    let proc_run = isolated_proc_run(root.path(), artifact_root.path(), isolation.clone());

    let read = run_command(&proc_run, "cat", vec!["seed.txt".to_string()]).await;
    assert_eq!(read["exitCode"], json!(0), "read result: {read}");
    assert_eq!(read["stdout"], json!("linked-folder-visible\n"));
    let write = run_command(&proc_run, "touch", vec!["positive.txt".to_string()]).await;
    assert_eq!(write["exitCode"], json!(0), "write result: {write}");
    assert!(root.path().join("positive.txt").is_file());

    let ambient = run_command(
        &proc_run,
        "test",
        vec!["-e".to_string(), "/etc/passwd".to_string()],
    )
    .await;
    assert_eq!(ambient["exitCode"], json!(1), "ambient result: {ambient}");

    let environment = run_command(&proc_run, "env", Vec::new()).await;
    assert_eq!(
        environment["exitCode"],
        json!(0),
        "environment result: {environment}"
    );
    let environment_stdout = environment["stdout"].as_str().unwrap();
    assert!(!environment_stdout.contains("TM_M4_AMBIENT_SECRET"));
    assert_eq!(
        environment_stdout
            .lines()
            .find(|line| line.starts_with("PATH=")),
        Some(format!("PATH={}", runtime_bin.display()).as_str())
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let listener_addr = listener.local_addr().unwrap();
    TcpStream::connect_timeout(&listener_addr, Duration::from_secs(1))
        .expect("outer process must be able to reach the canary listener");
    let network = run_command(
        &proc_run,
        "wget",
        vec![
            "-q".to_string(),
            "-T".to_string(),
            "1".to_string(),
            "-O".to_string(),
            "/dev/null".to_string(),
            format!("http://{listener_addr}/"),
        ],
    )
    .await;
    assert_ne!(network["exitCode"], json!(0), "network result: {network}");

    let limits = run_command(&proc_run, "cat", vec!["/proc/self/limits".to_string()]).await;
    assert_eq!(limits["exitCode"], json!(0), "limits result: {limits}");
    let nofile = limits["stdout"]
        .as_str()
        .unwrap()
        .lines()
        .find(|line| line.starts_with("Max open files"))
        .expect("/proc/self/limits omitted Max open files")
        .split_whitespace()
        .collect::<Vec<_>>();
    assert_eq!(&nofile[..5], &["Max", "open", "files", "64", "64"]);

    let capture = Arc::new(CaptureThenApprove::default());
    let required_profile_ctx =
        InvocationCtx::with_approvals(ctx().grants, capture.clone(), Duration::from_secs(1));
    let unavailable = isolated_proc_run(
        root.path(),
        artifact_root.path(),
        ProcIsolationConfig::LinuxBubblewrap {
            launcher: runtime_bin.join("missing-bwrap"),
            runtime_roots: vec![runtime_root],
            limits: ProcIsolationLimits::default(),
        },
    );
    let error = unavailable
        .call(
            json!({
                "cmd": "touch",
                "args": ["fallback-marker.txt"],
                "cwd": "tempestmiku:",
            }),
            &required_profile_ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, HostError::CapabilityDenied(_)));
    assert!(capture.actions.lock().unwrap().is_empty());
    assert!(!root.path().join("fallback-marker.txt").exists());

    // Hold the launcher after proc.run's final descriptor-relative validation but before
    // bubblewrap consumes its mount arguments. Replacing the path at this exact point used to
    // redirect the sandbox to the attacker's directory; `--bind-fd` must keep the approved inode.
    let root_race = tempfile::tempdir().unwrap();
    let approved_root = root_race.path().join("linked-root");
    fs::create_dir(&approved_root).unwrap();
    fs::write(approved_root.join("seed.txt"), "approved-root\n").unwrap();
    let root_race_artifacts = tempfile::tempdir().unwrap();
    let root_race_control = tempfile::tempdir().unwrap();
    let root_race_profile = delayed_launcher_profile(&isolation, root_race_control.path());
    let root_race_proc = Arc::new(isolated_proc_run(
        &approved_root,
        root_race_artifacts.path(),
        root_race_profile,
    ));
    let root_race_task = {
        let proc_run = Arc::clone(&root_race_proc);
        tokio::spawn(
            async move { run_command(&proc_run, "cat", vec!["seed.txt".to_string()]).await },
        )
    };
    wait_for_launcher(root_race_control.path()).await;
    let displaced_root = root_race.path().join("approved-root-displaced");
    fs::rename(&approved_root, &displaced_root).unwrap();
    fs::create_dir(&approved_root).unwrap();
    fs::write(approved_root.join("seed.txt"), "replacement-root\n").unwrap();
    release_launcher(root_race_control.path());
    let root_race_result = root_race_task.await.unwrap();
    assert_eq!(
        root_race_result["stdout"],
        json!("approved-root\n"),
        "root replacement result: {root_race_result}"
    );
    assert_eq!(
        fs::read_to_string(approved_root.join("seed.txt")).unwrap(),
        "replacement-root\n"
    );

    // Pin cwd separately from the root. With only the root descriptor pinned, replacing this
    // child entry before bubblewrap mounts would still redirect `--chdir` to the new directory.
    let cwd_race = tempfile::tempdir().unwrap();
    let cwd_race_root = cwd_race.path().join("linked-root");
    let approved_cwd = cwd_race_root.join("work");
    fs::create_dir_all(&approved_cwd).unwrap();
    fs::write(approved_cwd.join("seed.txt"), "approved-cwd\n").unwrap();
    let cwd_race_artifacts = tempfile::tempdir().unwrap();
    let cwd_race_control = tempfile::tempdir().unwrap();
    let cwd_race_profile = delayed_launcher_profile(&isolation, cwd_race_control.path());
    let cwd_race_proc = Arc::new(isolated_proc_run(
        &cwd_race_root,
        cwd_race_artifacts.path(),
        cwd_race_profile,
    ));
    let cwd_race_task = {
        let proc_run = Arc::clone(&cwd_race_proc);
        tokio::spawn(async move {
            run_command_in(
                &proc_run,
                "cat",
                vec!["seed.txt".to_string()],
                "tempestmiku:work",
            )
            .await
        })
    };
    wait_for_launcher(cwd_race_control.path()).await;
    let displaced_cwd = cwd_race_root.join("approved-cwd-displaced");
    fs::rename(&approved_cwd, &displaced_cwd).unwrap();
    fs::create_dir(&approved_cwd).unwrap();
    fs::write(approved_cwd.join("seed.txt"), "replacement-cwd\n").unwrap();
    release_launcher(cwd_race_control.path());
    let cwd_race_result = cwd_race_task.await.unwrap();
    assert_eq!(
        cwd_race_result["stdout"],
        json!("approved-cwd\n"),
        "cwd replacement result: {cwd_race_result}"
    );
    assert_eq!(
        fs::read_to_string(approved_cwd.join("seed.txt")).unwrap(),
        "replacement-cwd\n"
    );

    // Pin the executable itself, not merely its approved path identity. The delayed launcher opens
    // a deterministic window after proc.run's final validation and before bubblewrap consumes its
    // arguments. Replacing the executable path in that window must still run the approved inode.
    let executable_race_runtime = tempfile::tempdir().unwrap();
    let executable_race_bin = executable_race_runtime.path().join("bin");
    fs::create_dir(&executable_race_bin).unwrap();
    fs::set_permissions(
        executable_race_runtime.path(),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::set_permissions(&executable_race_bin, fs::Permissions::from_mode(0o755)).unwrap();
    let executable_path = executable_race_bin.join("race-exec");
    fs::write(
        &executable_path,
        format!(
            "#!{} sh\nprintf 'approved-executable\\n'\n",
            runtime_bin.join("busybox").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755)).unwrap();
    let race_path =
        std::env::join_paths([executable_race_bin.clone(), runtime_bin.clone()]).unwrap();
    let _path = PathGuard::install(race_path);

    let executable_race_root = tempfile::tempdir().unwrap();
    let executable_race_artifacts = tempfile::tempdir().unwrap();
    let executable_race_control = tempfile::tempdir().unwrap();
    let mut executable_race_isolation = isolation.clone();
    let ProcIsolationConfig::LinuxBubblewrap { runtime_roots, .. } = &mut executable_race_isolation
    else {
        unreachable!("gate always builds a Linux profile")
    };
    runtime_roots.insert(0, executable_race_runtime.path().to_path_buf());
    let executable_race_profile =
        delayed_launcher_profile(&executable_race_isolation, executable_race_control.path());
    let executable_race_proc = Arc::new(isolated_proc_run(
        executable_race_root.path(),
        executable_race_artifacts.path(),
        executable_race_profile,
    ));
    let executable_race_task = {
        let proc_run = Arc::clone(&executable_race_proc);
        tokio::spawn(async move { run_command(&proc_run, "race-exec", Vec::new()).await })
    };
    wait_for_launcher(executable_race_control.path()).await;
    let approved_executable = executable_race_bin.join("race-exec-approved");
    fs::rename(&executable_path, &approved_executable).unwrap();
    fs::write(
        &executable_path,
        format!(
            "#!{} sh\nprintf 'replacement-executable\\n'\n",
            runtime_bin.join("busybox").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755)).unwrap();
    release_launcher(executable_race_control.path());
    let executable_race_result = executable_race_task.await.unwrap();
    assert_eq!(
        executable_race_result["stdout"],
        json!("approved-executable\n"),
        "executable replacement result: {executable_race_result}"
    );
}
