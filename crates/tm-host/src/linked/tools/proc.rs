use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const DEFAULT_OUTPUT_BYTES: usize = 50_000;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_STDIN_BYTES: usize = 1024 * 1024;
const MAX_PROCESS_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETAINED_PROCESS_OUTPUT_BYTES: usize = MAX_PROCESS_ARTIFACT_BYTES - 256;

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
        let stdin = parse_stdin(args.stdin)?;
        if args.env.as_ref().is_some_and(|env| !env.is_empty()) {
            return Err(HostError::InvalidArgs(
                "proc.run env overrides are unavailable in P0".to_string(),
            ));
        }
        let cwd = self.linked.resolve_existing(args.cwd.as_deref())?;
        ctx.require_linked_alias(&cwd.alias)?;
        if !cwd.policy.commands.contains(&args.cmd) {
            return Err(HostError::CapabilityDenied(args.cmd.clone()));
        }
        let mut argv = vec![args.cmd.clone()];
        argv.extend(args.args.clone());
        let safe = cwd
            .policy
            .safe_args
            .iter()
            .any(|prefix| argv.starts_with(prefix));
        if !safe {
            ctx.require_approval(&format!("proc.run {}", argv.join(" ")))
                .await?;
        }
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
        let mut command = tokio::process::Command::new(&args.cmd);
        command
            .args(&args.args)
            .current_dir(&cwd.path)
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
        command.process_group(0);
        for (key, value) in env::vars() {
            let upper = key.to_uppercase();
            if ["KEY", "TOKEN", "SECRET", "PASSWORD", "COOKIE", "AUTH"]
                .iter()
                .any(|needle| upper.contains(needle))
            {
                continue;
            }
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|err| HostError::HostCall(err.to_string()))?;
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
                let artifact = self
                    .artifact_store
                    .put_text(
                        &persisted,
                        Some(format!("proc.run {}", args.cmd)),
                        "text/plain",
                    )
                    .map_err(|err| HostError::HostCall(err.to_string()))?;
                (
                    preview(&stdout, output_bytes),
                    preview(&stderr, output_bytes),
                    true,
                    Some(artifact),
                )
            } else {
                (stdout, stderr, false, None)
            };
        let result = ProcRunResult {
            cmd: args.cmd,
            args: args.args,
            cwd: display_path(&cwd.alias, &cwd.relative),
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

    #[tokio::test]
    async fn early_stdin_pipe_closure_is_not_a_process_failure() {
        let (writer, reader) = tokio::io::duplex(1);
        drop(reader);
        write_stdin(Some(writer), Some(vec![b'x'; 1024]))
            .await
            .unwrap();
    }
}
