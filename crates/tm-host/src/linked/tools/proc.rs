use super::*;

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
        let output_bytes = args.output_bytes.unwrap_or(50_000);
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
        let output =
            tokio::time::timeout(Duration::from_millis(timeout_ms), command.output()).await;
        let duration_ms = start.elapsed().as_millis();
        let (exit_code, timed_out, stdout, stderr) = match output {
            Ok(Ok(output)) => (
                output.status.code().unwrap_or(-1),
                false,
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ),
            Ok(Err(err)) => return Err(HostError::HostCall(err.to_string())),
            Err(_) => (
                -1,
                true,
                String::new(),
                "TimeoutError: proc.run timed out".to_string(),
            ),
        };
        let combined = format!("{stdout}{stderr}");
        let (stdout, stderr, truncated, artifact) = if combined.len() > output_bytes {
            let artifact = self
                .artifact_store
                .put_text(
                    &combined,
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
