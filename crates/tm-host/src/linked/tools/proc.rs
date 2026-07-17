use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const DEFAULT_OUTPUT_BYTES: usize = 50_000;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_STDIN_BYTES: usize = 1024 * 1024;
const MAX_STDIN_APPROVAL_PREVIEW_BYTES: usize = 256;
const MAX_ARG_COUNT: usize = 32;
const MAX_ARG_BYTES: usize = 2 * 1024;
const MAX_SINGLE_ARG_BYTES: usize = 512;
const MAX_PROCESS_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETAINED_PROCESS_OUTPUT_BYTES: usize = MAX_PROCESS_ARTIFACT_BYTES - 256;
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

pub(in crate::linked) struct ProcRunFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    timeout_ms: u64,
    docs: ToolDocs,
}

impl ProcRunFn {
    pub(in crate::linked) fn with_timeout_ms(
        linked: LinkedFolders,
        artifact_store: ArtifactStore,
        timeout_ms: u64,
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
        // Host process execution is not OS-isolated yet. Even an exact `cargo test` argv can run
        // repository-controlled build scripts or tests with the server's filesystem and network
        // authority, so configured safe args cannot bypass approval until that isolation exists.
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
        let (mut child, executed_cwd) =
            self.linked
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
                            "proc.run executable changed while approval was pending; retry"
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
                    #[cfg(unix)]
                    let mut command = {
                        let mut command = tokio::process::Command::new(&fresh_executable_target);
                        // Preserve multicall/proxy dispatch (for example rustup's `cargo` symlink)
                        // while executing the already-resolved canonical target rather than
                        // reopening the PATH symlink after approval.
                        command.arg0(&args.cmd);
                        command
                    };
                    #[cfg(not(unix))]
                    let mut command = tokio::process::Command::new(&fresh_executable);
                    command
                        .args(&args.args)
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
                        use std::os::unix::ffi::OsStrExt;

                        command.process_group(0);
                        let cwd_file = cwd_handle.file;
                        // Allocate and validate everything needed by the child-side identity check
                        // before fork. `pre_exec` is the last available hook before the path-based
                        // exec performed by `Command`.
                        let executable_path =
                            std::ffi::CString::new(fresh_executable_target.as_os_str().as_bytes())
                                .map_err(|_| {
                                    HostError::HostCall(
                                        "proc.run executable path contains a NUL byte".to_string(),
                                    )
                                })?;
                        let expected_device: libc::dev_t =
                            fresh_executable_identity.0.try_into().map_err(|_| {
                                HostError::HostCall(
                                    "proc.run executable device identity is not representable"
                                        .to_string(),
                                )
                            })?;
                        let expected_inode = fresh_executable_identity.1;
                        // SAFETY: the closure calls only descriptor/path syscalls with storage and
                        // a NUL-terminated path prepared before fork. It performs no heap allocation
                        // or locking. A mismatch returns ESTALE and prevents exec.
                        unsafe {
                            command.pre_exec(move || {
                                rustix::process::fchdir(&cwd_file).map_err(|error| {
                                    std::io::Error::from_raw_os_error(error.raw_os_error())
                                })?;
                                let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                                // SAFETY: `executable_path` is a live CString and `stat` points to
                                // writable storage for one `libc::stat` result.
                                if libc::stat(executable_path.as_ptr(), stat.as_mut_ptr()) != 0 {
                                    return Err(std::io::Error::last_os_error());
                                }
                                // SAFETY: `libc::stat` returned success and initialized the value.
                                let stat = stat.assume_init();
                                if stat.st_dev != expected_device || stat.st_ino != expected_inode {
                                    return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
                                }
                                Ok(())
                            });
                        }
                    }
                    #[cfg(not(unix))]
                    command.current_dir(fresh_cwd.policy.root.join(&fresh_cwd.relative));
                    for (key, value) in &inherited_environment {
                        command.env(key, value);
                    }
                    let child = command
                        .spawn()
                        .map_err(|err| HostError::HostCall(err.to_string()))?;
                    Ok((child, display_path(&fresh_cwd.alias, &fresh_cwd.relative)))
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

fn bounded_inline_output(stdout: &str, stderr: &str, limit: usize) -> (String, String) {
    let stdout_end = utf8_prefix_len(stdout, limit);
    let stdout = stdout[..stdout_end].to_string();
    let remaining = limit.saturating_sub(stdout.len());
    let stderr_end = utf8_prefix_len(stderr, remaining);
    let stderr = stderr[..stderr_end].to_string();
    (stdout, stderr)
}

fn utf8_prefix_len(value: &str, limit: usize) -> usize {
    let mut end = value.len().min(limit);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn stdin_approval_preview(stdin: Option<&[u8]>) -> Result<(Option<String>, bool)> {
    let Some(stdin) = stdin else {
        return Ok((None, false));
    };
    let stdin = std::str::from_utf8(stdin)
        .map_err(|_| HostError::HostCall("validated proc.run stdin was not UTF-8".to_string()))?;
    let redacted = tm_memory::redact_dream_text(stdin).text;
    if redacted.len() <= MAX_STDIN_APPROVAL_PREVIEW_BYTES {
        return Ok((Some(redacted), false));
    }

    let marker = format!("...[truncated:{} bytes]", redacted.len());
    let prefix_limit = MAX_STDIN_APPROVAL_PREVIEW_BYTES.saturating_sub(marker.len());
    let prefix_end = utf8_prefix_len(&redacted, prefix_limit);
    Ok((Some(format!("{}{}", &redacted[..prefix_end], marker)), true))
}

fn inherited_environment() -> Result<Vec<(String, std::ffi::OsString)>> {
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

fn sanitize_path(raw_path: &std::ffi::OsStr) -> Result<std::ffi::OsString> {
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

fn resolve_executable(
    command: &str,
    sanitized_path: &std::ffi::OsStr,
) -> Result<(std::path::PathBuf, std::path::PathBuf, (u64, u64))> {
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

struct ProcessGroupGuard {
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl ProcessGroupGuard {
    fn new(child_id: Option<u32>) -> Self {
        #[cfg(not(unix))]
        let _ = child_id;
        Self {
            #[cfg(unix)]
            process_group: child_id.and_then(|id| i32::try_from(id).ok()),
        }
    }

    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.process_group = None;
        }
    }

    #[cfg(unix)]
    fn kill(&self) -> std::io::Result<()> {
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        // SAFETY: `process_group` comes from the spawned child PID and the command was placed in a
        // new process group before spawning. A negative PID targets that entire group.
        let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = self.kill();
    }
}

async fn stop_process_tree(
    child: &mut tokio::process::Child,
    process_group: &mut ProcessGroupGuard,
) -> std::io::Result<()> {
    #[cfg(unix)]
    process_group.kill()?;
    #[cfg(not(unix))]
    child.kill().await?;

    match child.wait().await {
        Ok(_) => {
            process_group.disarm();
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            process_group.disarm();
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn parse_stdin(stdin: Option<Value>) -> Result<Option<Vec<u8>>> {
    match stdin {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(stdin)) => {
            if stdin.len() > MAX_STDIN_BYTES {
                return Err(HostError::InvalidArgs(format!(
                    "proc.run stdin must not exceed {MAX_STDIN_BYTES} UTF-8 bytes"
                )));
            }
            Ok(Some(stdin.into_bytes()))
        }
        Some(_) => Err(HostError::InvalidArgs(
            "proc.run stdin must be a UTF-8 string".to_string(),
        )),
    }
}

async fn write_stdin<W>(stdin: Option<W>, data: Option<Vec<u8>>) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    match (stdin, data) {
        (Some(mut stdin), Some(data)) => {
            if let Err(error) = stdin.write_all(&data).await {
                return if error.kind() == std::io::ErrorKind::BrokenPipe {
                    Ok(())
                } else {
                    Err(error)
                };
            }
            match stdin.shutdown().await {
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                result => result,
            }
        }
        _ => Ok(()),
    }
}

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded_output<R>(
    mut reader: R,
    retained: Arc<AtomicUsize>,
    limit: usize,
) -> std::io::Result<BoundedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let keep = reserve_output_bytes(&retained, read, limit);
        bytes.extend_from_slice(&chunk[..keep]);
        truncated |= keep < read;
    }
    Ok(BoundedOutput { bytes, truncated })
}

fn reserve_output_bytes(retained: &AtomicUsize, requested: usize, limit: usize) -> usize {
    loop {
        let current = retained.load(Ordering::Relaxed);
        let keep = requested.min(limit.saturating_sub(current));
        if keep == 0 {
            return 0;
        }
        if retained
            .compare_exchange_weak(
                current,
                current + keep,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            return keep;
        }
    }
}

fn bounded_process_artifact(mut output: String, output_limit_reached: bool) -> String {
    let marker = if output_limit_reached || output.len() > MAX_PROCESS_ARTIFACT_BYTES {
        format!(
            "\n… proc.run retained-output limit reached at {MAX_RETAINED_PROCESS_OUTPUT_BYTES} bytes …"
        )
    } else {
        String::new()
    };
    let content_cap = MAX_PROCESS_ARTIFACT_BYTES.saturating_sub(marker.len());
    if output.len() > content_cap {
        let mut end = content_cap;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }
    output.push_str(&marker);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_process_output_budget_never_exceeds_the_limit() {
        let retained = AtomicUsize::new(0);
        assert_eq!(reserve_output_bytes(&retained, 7, 10), 7);
        assert_eq!(reserve_output_bytes(&retained, 7, 10), 3);
        assert_eq!(reserve_output_bytes(&retained, 1, 10), 0);
        assert_eq!(retained.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn process_artifact_keeps_a_bounded_truncation_marker() {
        let output = bounded_process_artifact("x".repeat(MAX_PROCESS_ARTIFACT_BYTES), true);
        assert!(output.len() <= MAX_PROCESS_ARTIFACT_BYTES);
        assert!(output.contains("retained-output limit reached"));
    }

    #[test]
    fn inline_stdout_and_stderr_share_one_budget() {
        let (stdout, stderr) = bounded_inline_output("12345678", "abcdef", 10);
        assert_eq!(stdout, "12345678");
        assert_eq!(stderr, "ab");
        assert_eq!(stdout.len() + stderr.len(), 10);

        let (stdout, stderr) = bounded_inline_output("世界", "界", 4);
        assert_eq!(stdout, "世");
        assert_eq!(stderr, "");
    }

    #[tokio::test]
    async fn early_stdin_pipe_closure_is_not_a_process_failure() {
        let (writer, reader) = tokio::io::duplex(1);
        drop(reader);
        write_stdin(Some(writer), Some(vec![b'x'; 1024]))
            .await
            .unwrap();
    }

    #[test]
    fn proc_path_drops_relative_and_empty_search_entries() {
        let absolute = tempfile::tempdir().unwrap();
        let raw = env::join_paths([
            std::path::PathBuf::from("."),
            std::path::PathBuf::new(),
            absolute.path().to_path_buf(),
        ])
        .unwrap();
        let sanitized = sanitize_path(&raw).unwrap();
        let entries = env::split_paths(&sanitized).collect::<Vec<_>>();
        assert_eq!(entries, vec![absolute.path().canonicalize().unwrap()]);
        assert!(entries.iter().all(|path| path.is_absolute()));
    }
}
