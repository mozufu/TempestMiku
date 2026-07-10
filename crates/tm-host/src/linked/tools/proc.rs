use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt};

const DEFAULT_OUTPUT_BYTES: usize = 50_000;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_PROCESS_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETAINED_PROCESS_OUTPUT_BYTES: usize = MAX_PROCESS_ARTIFACT_BYTES - 256;

pub(in crate::linked) struct ProcRunFn {
    linked: LinkedFolders,
    artifact_store: ArtifactStore,
    docs: ToolDocs,
}

impl ProcRunFn {
    pub(in crate::linked) fn new(linked: LinkedFolders, artifact_store: ArtifactStore) -> Self {
        Self {
            linked,
            artifact_store,
            docs: docs(
                "proc.run",
                "proc",
                "Run allowlisted argv-vector commands",
                true,
            ),
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
        if stdin_present(&args.stdin) {
            return Err(HostError::InvalidArgs(
                "proc.run stdin is unavailable in P0".to_string(),
            ));
        }
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
        let timeout_ms = args.timeout_ms.unwrap_or(180_000).min(180_000);
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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
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
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HostError::HostCall("proc.run stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HostError::HostCall("proc.run stderr pipe missing".to_string()))?;
        let retained = Arc::new(AtomicUsize::new(0));
        let run = async {
            tokio::join!(
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
            Ok((Ok(status), Ok(stdout), Ok(stderr))) => (
                status.code().unwrap_or(-1),
                false,
                String::from_utf8_lossy(&stdout.bytes).to_string(),
                String::from_utf8_lossy(&stderr.bytes).to_string(),
                stdout.truncated || stderr.truncated,
            ),
            Ok((Err(err), _, _)) | Ok((_, Err(err), _)) | Ok((_, _, Err(err))) => {
                return Err(HostError::HostCall(err.to_string()));
            }
            Err(_) => (
                -1,
                true,
                String::new(),
                "TimeoutError: proc.run timed out".to_string(),
                false,
            ),
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
}
