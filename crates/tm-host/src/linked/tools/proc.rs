use super::*;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::atomic::AtomicUsize};

#[cfg(unix)]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd};

pub(in crate::linked::tools) mod bounded_io;
pub(in crate::linked::tools) mod environment;
pub(in crate::linked::tools) mod process_group;

use super::super::isolation::{PreparedProcIsolation, ProcIsolationCommand, ProcIsolationConfig};
use bounded_io::{
    MAX_RETAINED_PROCESS_OUTPUT_BYTES, bounded_inline_output, bounded_process_artifact,
    parse_stdin, read_bounded_output, stdin_approval_preview, write_stdin,
};
use environment::{inherited_environment, resolve_executable};
use process_group::{ProcessGroupGuard, stop_process_tree};

const DEFAULT_OUTPUT_BYTES: usize = 50_000;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_ARG_COUNT: usize = 32;
const MAX_ARG_BYTES: usize = 2 * 1024;
const MAX_SINGLE_ARG_BYTES: usize = 512;

pub(in crate::linked) struct ProcRunFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    timeout_ms: u64,
    isolation: ProcIsolationConfig,
    docs: ToolDocs,
}

impl ProcRunFn {
    #[cfg(test)]
    pub(in crate::linked) fn with_timeout_ms(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        timeout_ms: u64,
    ) -> Self {
        Self::with_timeout_and_isolation(
            linked,
            artifact_store,
            timeout_ms,
            ProcIsolationConfig::default(),
        )
    }

    pub(in crate::linked) fn with_timeout_and_isolation(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        timeout_ms: u64,
        isolation: ProcIsolationConfig,
    ) -> Self {
        let timeout_ms = timeout_ms.clamp(1, 900_000);
        let mut docs = docs(
            "proc.run",
            "proc",
            "Run allowlisted argv-vector commands",
            true,
        );
        docs.args_schema["properties"]["timeoutMs"]["maximum"] = timeout_ms.into();
        docs.args_schema["properties"]["timeoutMs"]["default"] = timeout_ms.into();
        Self {
            linked,
            artifact_store,
            timeout_ms,
            isolation,
            docs,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcRunResult {
    cmd: String,
    args: Vec<String>,
    cwd: String,
    exit_code: i32,
    timed_out: bool,
    stdout: String,
    stderr: String,
    truncated: bool,
    artifact: Option<ArtifactRef>,
    duration_ms: u128,
}

#[async_trait]
impl HostFn for ProcRunFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            cmd: String,
            #[serde(default)]
            args: Vec<String>,
            cwd: Option<String>,
            timeout_ms: Option<u64>,
            output_bytes: Option<usize>,
            stdin: Option<Value>,
            env: Option<BTreeMap<String, Value>>,
        }
        let args: Args = parse_args(args)?;
        validate_command_name(&args.cmd)?;
        let argument_bytes = args
            .args
            .iter()
            .fold(0_usize, |total, arg| total.saturating_add(arg.len()));
        if args.args.len() > MAX_ARG_COUNT
            || args.cmd.len() > MAX_SINGLE_ARG_BYTES
            || args.cmd.len().saturating_add(argument_bytes) > MAX_ARG_BYTES
            || args.args.iter().any(|arg| arg.len() > MAX_SINGLE_ARG_BYTES)
        {
            return Err(HostError::InvalidArgs(format!(
                "proc.run command and args must fit the approval prompt: at most {MAX_ARG_COUNT} argument entries, {MAX_SINGLE_ARG_BYTES} bytes per entry, and {MAX_ARG_BYTES} UTF-8 bytes total"
            )));
        }
        let stdin = parse_stdin(args.stdin)?;
        if args.env.as_ref().is_some_and(|env| !env.is_empty()) {
            return Err(HostError::InvalidArgs(
                "proc.run env overrides are unavailable in P0".to_string(),
            ));
        }
        let inherited_environment = inherited_environment()?;
        let sanitized_path = inherited_environment
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                HostError::CapabilityDenied(
                    "proc.run PATH has no absolute executable search directories".to_string(),
                )
            })?;
        let (resolved_executable, executable_target, executable_identity) =
            resolve_executable(&args.cmd, &sanitized_path)?;
        #[cfg(unix)]
        let pinned_executable = pin_executable(&executable_target, executable_identity)?;
        // An enabled profile is a hard requirement, not a preference. Resolve and pin it before
        // approval so an unavailable launcher or mount profile never falls back to host execution.
        let prepared_isolation = self.isolation.prepare()?;
        let revision = self.linked.revision();
        let requested_cwd = self.linked.resolve_spec(args.cwd.as_deref())?;
        ctx.require_linked_alias(&requested_cwd.alias)?;
        let stable_cwd_path = display_path(&requested_cwd.alias, &requested_cwd.relative);
        let (cwd, initial_cwd) = self
            .linked
            .with_stable_policy_snapshot(revision, |linked| {
                let cwd = linked.resolve_spec(Some(&stable_cwd_path))?;
                if !cwd.policy.commands.contains(&args.cmd) {
                    return Err(HostError::CapabilityDenied(args.cmd.clone()));
                }
                let initial_cwd =
                    open_existing(&cwd.policy, cwd.root_identity, &cwd.relative, &cwd.display)?;
                if initial_cwd.kind != SecureKind::Directory {
                    return Err(HostError::InvalidPath(format!(
                        "{} is not a directory",
                        cwd.display
                    )));
                }
                Ok((cwd, initial_cwd))
            })?;
        // Approval remains mandatory in both profiles: repository-controlled build scripts can
        // still mutate the granted linked folder even when Linux namespaces remove ambient host
        // filesystem/network authority. The disabled profile additionally exposes host authority.
        let mut exact_argv = vec![args.cmd.clone()];
        exact_argv.extend(args.args.clone());
        let argv_bytes =
            serde_json::to_vec(&exact_argv).map_err(|err| HostError::HostCall(err.to_string()))?;
        let argv_sha256 = hex::encode(Sha256::digest(&argv_bytes));
        let stdin_present = stdin.is_some();
        let stdin_bytes = stdin.as_ref().map(Vec::len).unwrap_or(0);
        let stdin_sha256 = stdin
            .as_deref()
            .map(|bytes| hex::encode(Sha256::digest(bytes)));
        let (stdin_preview, stdin_preview_truncated) = stdin_approval_preview(stdin.as_deref())?;
        let approval = approval_action(
            "proc.run",
            serde_json::json!({
                "argvPreview": exact_argv,
                "argvSha256": argv_sha256,
                "argvEncodedBytes": argv_bytes.len(),
                "resolvedExecutable": resolved_executable.clone(),
                "executableTarget": executable_target.clone(),
                "executableDevice": executable_identity.0,
                "executableInode": executable_identity.1,
                "cwd": {
                    "linkedPath": display_path(&cwd.alias, &cwd.relative),
                    "device": initial_cwd.identity.device,
                    "inode": initial_cwd.identity.inode,
                },
                "timeoutMs": args.timeout_ms.unwrap_or(self.timeout_ms).min(self.timeout_ms),
                "stdinPresent": stdin_present,
                "stdinBytes": stdin_bytes,
                "stdinSha256": stdin_sha256,
                "stdinPreview": stdin_preview,
                "stdinPreviewTruncated": stdin_preview_truncated,
                "isolation": prepared_isolation.approval_details(),
            }),
        );
        let approval_json: Value = serde_json::from_str(&approval)
            .map_err(|err| HostError::HostCall(format!("invalid proc.run approval JSON: {err}")))?;
        if approval_json["details"]["bounded"] == Value::Bool(true) {
            return Err(HostError::InvalidArgs(
                "proc.run arguments cannot be represented safely in the bounded approval prompt"
                    .to_string(),
            ));
        }
        ctx.require_approval(&approval).await?;
        let timeout_ms = args
            .timeout_ms
            .unwrap_or(self.timeout_ms)
            .min(self.timeout_ms);
        let output_bytes = args.output_bytes.unwrap_or(DEFAULT_OUTPUT_BYTES);
        if output_bytes == 0 || output_bytes > MAX_OUTPUT_BYTES {
            return Err(HostError::InvalidArgs(format!(
                "proc.run outputBytes must be between 1 and {MAX_OUTPUT_BYTES}"
            )));
        }
        let start = Instant::now();
        let initial_cwd_identity = initial_cwd.identity;
        let (mut child, executed_cwd, mut isolation_run) = self
            .linked
            .with_stable_policy_snapshot(revision, |linked| {
                let fresh_cwd = linked.resolve_spec(Some(&stable_cwd_path))?;
                if !fresh_cwd.policy.commands.contains(&args.cmd) {
                    return Err(HostError::CapabilityDenied(args.cmd.clone()));
                }
                let (fresh_executable, fresh_executable_target, fresh_executable_identity) =
                    resolve_executable(&args.cmd, &sanitized_path)?;
                if fresh_executable != resolved_executable
                    || fresh_executable_target != executable_target
                    || fresh_executable_identity != executable_identity
                {
                    return Err(HostError::InvalidArgs(
                        "proc.run executable changed while approval was pending; retry".to_string(),
                    ));
                }
                #[cfg(unix)]
                {
                    // Open the current path again while the policy snapshot is held. Comparing
                    // the descriptor identity closes the validation-side path race; execution
                    // still uses the descriptor pinned before approval, so a later rename can
                    // never substitute a different executable.
                    let fresh_pin =
                        pin_executable(&fresh_executable_target, fresh_executable_identity)?;
                    if fresh_pin.identity != pinned_executable.identity {
                        return Err(HostError::InvalidArgs(
                            "proc.run executable changed while approval was pending; retry"
                                .to_string(),
                        ));
                    }
                }
                let fresh_isolation = self.isolation.prepare()?;
                if fresh_isolation != prepared_isolation {
                    return Err(HostError::CapabilityDenied(
                        "proc isolation profile changed while approval was pending; retry"
                            .to_string(),
                    ));
                }
                let cwd_handle = open_existing(
                    &fresh_cwd.policy,
                    fresh_cwd.root_identity,
                    &fresh_cwd.relative,
                    &fresh_cwd.display,
                )?;
                if cwd_handle.kind != SecureKind::Directory
                    || cwd_handle.identity != initial_cwd_identity
                {
                    return Err(HostError::InvalidArgs(format!(
                        "proc.run cwd {} changed while approval was pending; retry",
                        fresh_cwd.display
                    )));
                }
                let executed_cwd_path = fresh_cwd.policy.root.join(&fresh_cwd.relative);
                // Allocate and fully configure the unpredictable cgroup leaf only after
                // approval, but before spawn. The profile itself was already probed before
                // approval, and `fresh_isolation` was compared with that exact approved
                // profile above.
                let isolation_run = fresh_isolation.start_run()?;
                #[cfg(target_os = "linux")]
                let cgroup_procs_file = isolation_run.cgroup_procs_file();
                let direct_execution = matches!(fresh_isolation, PreparedProcIsolation::Disabled);
                #[cfg(unix)]
                let linked_root_handle = if direct_execution {
                    None
                } else {
                    Some(open_existing(
                        &fresh_cwd.policy,
                        fresh_cwd.root_identity,
                        Path::new(""),
                        &fresh_cwd.alias,
                    )?)
                };
                #[cfg(unix)]
                let mut command = if direct_execution {
                    #[cfg(target_os = "linux")]
                    let mut command = tokio::process::Command::new(pinned_fd_path(
                        pinned_executable.file.as_raw_fd(),
                    ));
                    #[cfg(not(target_os = "linux"))]
                    let mut command = tokio::process::Command::new(&fresh_executable_target);
                    // Preserve multicall/proxy dispatch (for example rustup's `cargo` symlink)
                    // while Linux executes the descriptor pinned before approval. Other Unix
                    // hosts retain the final child-side identity check below because their
                    // `/dev/fd` implementation is not executable.
                    command.arg0(&args.cmd);
                    command
                } else {
                    use std::os::fd::AsRawFd;

                    let linked_root_fd = linked_root_handle
                        .as_ref()
                        .map(|handle| handle.file.as_raw_fd())
                        .ok_or_else(|| {
                            HostError::HostCall(
                                "proc isolation linked-root descriptor is missing".to_string(),
                            )
                        })?;
                    fresh_isolation.command(ProcIsolationCommand {
                        resolved_executable: &fresh_executable,
                        executable_target: &fresh_executable_target,
                        command_name: &args.cmd,
                        args: &args.args,
                        linked_root: &fresh_cwd.policy.root,
                        linked_root_fd,
                        linked_mode: fresh_cwd.policy.mode,
                        cwd: &executed_cwd_path,
                        cwd_fd: cwd_handle.file.as_raw_fd(),
                        executable_fd: pinned_executable.file.as_raw_fd(),
                        environment: &inherited_environment,
                    })?
                };
                #[cfg(not(unix))]
                let mut command = tokio::process::Command::new(&fresh_executable);
                if direct_execution {
                    command.args(&args.args);
                }
                command
                    .stdin(if stdin.is_some() {
                        Stdio::piped()
                    } else {
                        Stdio::null()
                    })
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .env_clear();
                #[cfg(unix)]
                {
                    use std::os::fd::AsRawFd;
                    #[cfg(not(target_os = "linux"))]
                    use std::os::unix::ffi::OsStrExt;

                    command.process_group(0);
                    let cwd_file = cwd_handle.file;
                    let linked_root_file = linked_root_handle.map(|handle| handle.file);
                    #[cfg(target_os = "linux")]
                    let executable_file = pinned_executable.file;
                    let child_isolation = fresh_isolation.clone();
                    #[cfg(not(target_os = "linux"))]
                    let executable_path =
                        std::ffi::CString::new(fresh_executable_target.as_os_str().as_bytes())
                            .map_err(|_| {
                                HostError::HostCall(
                                    "proc.run executable path contains a NUL byte".to_string(),
                                )
                            })?;
                    #[cfg(not(target_os = "linux"))]
                    let expected_identity = fresh_executable_identity;
                    // The descriptor is kept alive by this closure and made inheritable only in
                    // the forked child. Direct execution opens `/proc/self/fd/N` (or `/dev/fd/N`
                    // on other Unix systems); bubblewrap consumes the same descriptor with
                    // `--ro-bind-fd`. Both routes therefore execute the approved inode.
                    unsafe {
                        command.pre_exec(move || {
                            let pinned_mount_fds = linked_root_file
                                .as_ref()
                                .map(|root| (root.as_raw_fd(), cwd_file.as_raw_fd()));
                            if direct_execution {
                                rustix::process::fchdir(&cwd_file).map_err(|error| {
                                    std::io::Error::from_raw_os_error(error.raw_os_error())
                                })?;
                            }
                            #[cfg(target_os = "linux")]
                            child_isolation.prepare_child(
                                Some(executable_file.as_raw_fd()),
                                pinned_mount_fds,
                                cgroup_procs_file.as_ref().map(|file| file.as_raw_fd()),
                            )?;
                            #[cfg(not(target_os = "linux"))]
                            {
                                let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                                if libc::stat(executable_path.as_ptr(), stat.as_mut_ptr()) != 0 {
                                    return Err(std::io::Error::last_os_error());
                                }
                                let stat = stat.assume_init();
                                // libc identity widths vary across Unix targets; on some
                                // hosts one or both casts are intentionally a no-op.
                                #[allow(clippy::unnecessary_cast)]
                                let actual_identity = (stat.st_dev as u64, stat.st_ino as u64);
                                if actual_identity != expected_identity {
                                    return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
                                }
                                child_isolation.prepare_child(None, pinned_mount_fds, None)?;
                            }
                            Ok(())
                        });
                    }
                }
                #[cfg(not(unix))]
                command.current_dir(&executed_cwd_path);
                if direct_execution {
                    for (key, value) in &inherited_environment {
                        command.env(key, value);
                    }
                }
                let child = command
                    .spawn()
                    .map_err(|err| HostError::HostCall(err.to_string()))?;
                Ok((
                    child,
                    display_path(&fresh_cwd.alias, &fresh_cwd.relative),
                    isolation_run,
                ))
            })?;
        let mut process_group = ProcessGroupGuard::new(child.id());
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HostError::HostCall("proc.run stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HostError::HostCall("proc.run stderr pipe missing".to_string()))?;
        let child_stdin = child.stdin.take();
        let retained = Arc::new(AtomicUsize::new(0));
        let run = async {
            tokio::join!(
                write_stdin(child_stdin, stdin),
                child.wait(),
                read_bounded_output(
                    stdout,
                    Arc::clone(&retained),
                    MAX_RETAINED_PROCESS_OUTPUT_BYTES,
                ),
                read_bounded_output(
                    stderr,
                    Arc::clone(&retained),
                    MAX_RETAINED_PROCESS_OUTPUT_BYTES,
                ),
            )
        };
        let output = tokio::time::timeout(Duration::from_millis(timeout_ms), run).await;
        let duration_ms = start.elapsed().as_millis();
        let (exit_code, timed_out, stdout, stderr, output_limit_reached) = match output {
            Ok((Ok(()), Ok(status), Ok(stdout), Ok(stderr))) => {
                isolation_run.terminate_and_cleanup()?;
                process_group.disarm();
                (
                    status.code().unwrap_or(-1),
                    false,
                    String::from_utf8_lossy(&stdout.bytes).to_string(),
                    String::from_utf8_lossy(&stderr.bytes).to_string(),
                    stdout.truncated || stderr.truncated,
                )
            }
            Ok((Err(err), _, _, _))
            | Ok((_, Err(err), _, _))
            | Ok((_, _, Err(err), _))
            | Ok((_, _, _, Err(err))) => return Err(HostError::HostCall(err.to_string())),
            Err(_) => {
                isolation_run.terminate_and_cleanup()?;
                stop_process_tree(&mut child, &mut process_group)
                    .await
                    .map_err(|err| HostError::HostCall(err.to_string()))?;
                (
                    -1,
                    true,
                    String::new(),
                    "TimeoutError: proc.run timed out".to_string(),
                    false,
                )
            }
        };
        // Command output may contain credentials inherited from tools or repository files. The
        // process result remains useful after redaction, while no raw output crosses an artifact
        // or event persistence boundary.
        let stdout = tm_memory::redact_dream_text(&stdout).text;
        let stderr = tm_memory::redact_dream_text(&stderr).text;
        let combined = format!("{stdout}{stderr}");
        let (stdout, stderr, truncated, artifact) =
            if output_limit_reached || combined.len() > output_bytes {
                let persisted = bounded_process_artifact(combined, output_limit_reached);
                let artifact_store = self.artifact_store.clone();
                let title = format!("proc.run {}", args.cmd);
                let artifact = tokio::task::spawn_blocking(move || {
                    artifact_store
                        .put_text(&persisted, Some(title), "text/plain")
                        .map_err(|err| HostError::HostCall(err.to_string()))
                })
                .await
                .map_err(|err| {
                    HostError::HostCall(format!("proc.run artifact worker failed: {err}"))
                })??;
                let (stdout, stderr) = bounded_inline_output(&stdout, &stderr, output_bytes);
                (stdout, stderr, true, Some(artifact))
            } else {
                (stdout, stderr, false, None)
            };
        let result = ProcRunResult {
            cmd: args.cmd,
            args: args.args,
            cwd: executed_cwd,
            exit_code,
            timed_out,
            stdout,
            stderr,
            truncated,
            artifact,
            duration_ms,
        };
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

#[cfg(unix)]
struct PinnedExecutable {
    file: File,
    identity: (u64, u64),
}

#[cfg(unix)]
fn pin_executable(path: &Path, expected_identity: (u64, u64)) -> Result<PinnedExecutable> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[cfg(target_os = "linux")]
    let file = {
        use std::os::unix::ffi::OsStrExt;

        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            HostError::HostCall("proc.run executable path contains a NUL byte".to_string())
        })?;
        // O_PATH pins the executable without requiring read permission. The child later executes
        // the descriptor through procfs (direct) or binds it into the bubblewrap namespace.
        // SAFETY: `path` is a live NUL-terminated string and flags require no mode argument.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(HostError::CapabilityDenied(format!(
                "proc.run executable cannot be pinned: {}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: `open` returned a new owned descriptor.
        unsafe { File::from_raw_fd(fd) }
    };
    #[cfg(not(target_os = "linux"))]
    let file = File::open(path).map_err(|error| {
        HostError::CapabilityDenied(format!(
            "proc.run executable cannot be pinned on this Unix host: {error}"
        ))
    })?;

    let metadata = file.metadata().map_err(|error| {
        HostError::CapabilityDenied(format!(
            "proc.run pinned executable cannot be inspected: {error}"
        ))
    })?;
    let identity = (metadata.dev(), metadata.ino());
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || identity != expected_identity
    {
        return Err(HostError::InvalidArgs(
            "proc.run executable changed while it was being pinned; retry".to_string(),
        ));
    }
    Ok(PinnedExecutable { file, identity })
}

#[cfg(target_os = "linux")]
fn pinned_fd_path(fd: libc::c_int) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/proc/self/fd/{fd}"))
}
