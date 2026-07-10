use std::{
    cell::Cell,
    collections::BTreeMap,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions, serde_v8, v8};
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;
use tm_core::{CancellationToken, CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};
use tm_drive::SharedDriveStore;
use tm_host::{
    ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants, DefaultDenyApprovalPolicy,
    HostEventSink, HostRegistry, InvocationCtx, LinkedFolders, NoopHostEventSink, ResourceRegistry,
    register_p0_linked_folder_functions,
};

use crate::{
    ops::{HttpGetFn, RuntimeHostState, init_ops},
    prelude::{AGENTS_PRELUDE, SDK_PRELUDE},
    ts::compile_cell,
};

pub const CORE_SANDBOX_CAPABILITIES: &[&str] = &["http.get", "resources.read:artifact"];

pub fn core_sandbox_grants() -> CapabilityGrants {
    CapabilityGrants::default().allow_many(CORE_SANDBOX_CAPABILITIES.iter().copied())
}

/// Hard resource ceilings for one Deno isolate and cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DenoResourceLimits {
    /// Maximum V8 heap size for one isolate.
    pub heap_bytes: usize,
    /// Maximum retained stdout/result/error bytes before the cell fails closed.
    pub retained_output_bytes: usize,
    /// Maximum display calls retained for one cell.
    pub displays_per_cell: usize,
}

impl Default for DenoResourceLimits {
    fn default() -> Self {
        Self {
            heap_bytes: 128 * 1024 * 1024,
            retained_output_bytes: 4 * 1024 * 1024,
            displays_per_cell: 64,
        }
    }
}

/// Configuration for [`DenoSandbox`].
#[derive(Clone)]
pub struct DenoSandboxOptions {
    pub artifact_root: PathBuf,
    pub session_id: String,
    pub actor_id: Option<String>,
    pub session_scope: Option<String>,
    pub http_allowlist: BTreeMap<String, String>,
    pub host_registry: HostRegistry,
    pub resource_registry: ResourceRegistry,
    pub grants: CapabilityGrants,
    pub linked_folders: Option<LinkedFolders>,
    pub drive_store: Option<SharedDriveStore>,
    pub approval_policy: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
    pub host_event_sink: Arc<dyn HostEventSink>,
    pub cancellation: Option<Arc<dyn CancellationToken>>,
    pub limits: DenoResourceLimits,
}

impl Default for DenoSandboxOptions {
    fn default() -> Self {
        Self {
            artifact_root: tm_artifacts::default_root(),
            session_id: "default".to_string(),
            actor_id: None,
            session_scope: None,
            http_allowlist: BTreeMap::new(),
            host_registry: HostRegistry::new(),
            resource_registry: ResourceRegistry::new(),
            grants: core_sandbox_grants(),
            linked_folders: None,
            drive_store: None,
            approval_policy: Arc::new(DefaultDenyApprovalPolicy),
            approval_timeout: Duration::from_secs(60),
            host_event_sink: Arc::new(NoopHostEventSink),
            cancellation: None,
            limits: DenoResourceLimits::default(),
        }
    }
}

/// A `deno_core`-backed persistent JavaScript/TypeScript sandbox.
#[derive(Clone, Default)]
pub struct DenoSandbox {
    options: DenoSandboxOptions,
}

impl DenoSandbox {
    pub fn new(options: DenoSandboxOptions) -> Self {
        Self { options }
    }
}

#[async_trait]
impl Sandbox for DenoSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(DenoSession::new(self.options.clone())?))
    }
}

pub struct DenoSession {
    runtime: Option<JsRuntime>,
    artifact_store: ArtifactStore,
    options: DenoSandboxOptions,
    heap_exhausted: Rc<Cell<bool>>,
}

impl DenoSession {
    fn new(options: DenoSandboxOptions) -> Result<Self> {
        if options.limits.heap_bytes < 16 * 1024 * 1024 {
            return Err(tm_core::Error::Sandbox(
                "Deno heap limit must be at least 16 MiB".to_string(),
            ));
        }
        if options.limits.retained_output_bytes == 0
            || options.limits.retained_output_bytes > 4 * 1024 * 1024
        {
            return Err(tm_core::Error::Sandbox(
                "Deno retained output limit must be between 1 byte and 4 MiB".to_string(),
            ));
        }
        if options.limits.displays_per_cell == 0 || options.limits.displays_per_cell > 64 {
            return Err(tm_core::Error::Sandbox(
                "Deno display limit must be between 1 and 64".to_string(),
            ));
        }
        let artifact_store = ArtifactStore::open(&options.artifact_root, &options.session_id)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let mut host_registry = options.host_registry.clone();
        host_registry.register(Arc::new(HttpGetFn::new(options.http_allowlist.clone())));
        let mut resource_registry = options.resource_registry.clone();
        resource_registry.register(Arc::new(ArtifactResourceHandler::new(
            artifact_store.clone(),
        )));
        let grants = options.grants.clone();
        let linked_folders = options.linked_folders.clone().or_else(|| {
            options
                .drive_store
                .as_ref()
                .map(|_| LinkedFolders::default())
        });
        if let Some(linked_folders) = linked_folders.clone() {
            register_p0_linked_folder_functions(
                &mut host_registry,
                &mut resource_registry,
                linked_folders,
                artifact_store.clone(),
            );
        }
        if let Some(drive_store) = options.drive_store.clone() {
            tm_drive::register_drive_functions(
                &mut host_registry,
                &mut resource_registry,
                drive_store,
                linked_folders,
            );
        }
        let mut invocation_ctx = InvocationCtx::with_approvals(
            grants,
            options.approval_policy.clone(),
            options.approval_timeout,
        )
        .with_session_id(options.session_id.clone())
        .with_actor_id(options.actor_id.clone())
        .with_event_sink(options.host_event_sink.clone());
        if let Some(scope) = options.session_scope.clone() {
            invocation_ctx = invocation_ctx.with_session_scope(scope);
        }
        let host_state = RuntimeHostState {
            artifact_store: artifact_store.clone(),
            host_registry,
            resource_registry,
            invocation_ctx,
        };
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![init_ops(host_state)],
            create_params: Some(
                v8::Isolate::create_params().heap_limits(0, options.limits.heap_bytes),
            ),
            ..RuntimeOptions::default()
        });
        let heap_exhausted = Rc::new(Cell::new(false));
        let heap_exhausted_for_callback = Rc::clone(&heap_exhausted);
        let isolate_handle = runtime.v8_isolate().thread_safe_handle();
        runtime.add_near_heap_limit_callback(move |current_limit, _initial_limit| {
            heap_exhausted_for_callback.set(true);
            isolate_handle.terminate_execution();
            // V8 needs a small amount of headroom to unwind the terminated execution.
            current_limit.saturating_mul(2)
        });
        let mut session = Self {
            runtime: Some(runtime),
            artifact_store,
            options,
            heap_exhausted,
        };
        session.install_prelude()?;
        Ok(session)
    }

    fn runtime(&mut self) -> &mut JsRuntime {
        self.runtime
            .as_mut()
            .expect("Deno runtime missing outside reset")
    }

    fn install_prelude(&mut self) -> Result<()> {
        self.runtime()
            .execute_script("<tempestmiku-prelude>", SDK_PRELUDE)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        if self
            .options
            .grants
            .names()
            .any(|n| n.starts_with("agents."))
        {
            self.runtime()
                .execute_script("<tempestmiku-agents>", AGENTS_PRELUDE)
                .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        }
        Ok(())
    }
}

#[async_trait(?Send)]
impl Session for DenoSession {
    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput> {
        if self.cancelled() {
            return Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some("CancellationError: cell cancelled".to_string()),
            });
        }
        if budget.wall_ms == 0 {
            return Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some("TimeoutError: cell exceeded wall-clock budget".to_string()),
            });
        }

        if budget.output_bytes == 0 {
            return Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some("ResourceLimitError: cell output budget is zero".to_string()),
            });
        }
        let retained_output_limit = self.options.limits.retained_output_bytes;
        let shaped_output_limit = budget.output_bytes.min(retained_output_limit);
        let display_limit = self.options.limits.displays_per_cell;
        self.runtime()
            .execute_script(
                "<tempestmiku-clear>",
                format!(
                    "globalThis.__tm_stdout = []; globalThis.__tm_displays = []; \
                     globalThis.__tm_output_limit = {retained_output_limit}; \
                     globalThis.__tm_output_size = 0; \
                     globalThis.__tm_output_truncated = false; \
                     globalThis.__tm_display_limit = {display_limit}; \
                     globalThis.__tm_display_count = 0;"
                ),
            )
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;

        let code = match compile_cell(code) {
            Ok(code) => code,
            Err(err) => {
                return Ok(EvalOutput {
                    stdout: String::new(),
                    result: None,
                    error: Some(err.to_string()),
                });
            }
        };
        let (watchdog_stop_tx, watchdog_stop_rx) = mpsc::channel();
        let isolate_handle = self.runtime().v8_isolate().thread_safe_handle();
        let wall_ms = budget.wall_ms;
        let cancellation = self.options.cancellation.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(wall_ms);
            loop {
                if watchdog_stop_rx.try_recv().is_ok() {
                    break;
                }
                if cancellation
                    .as_ref()
                    .is_some_and(|token| token.is_cancelled())
                {
                    isolate_handle.terminate_execution();
                    break;
                }
                let now = Instant::now();
                if now >= deadline {
                    isolate_handle.terminate_execution();
                    break;
                }
                let sleep_for = deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(10));
                if watchdog_stop_rx.recv_timeout(sleep_for).is_ok() {
                    break;
                }
            }
        });

        let mut result = match self.runtime().execute_script("<cell>", code) {
            Ok(global) => {
                let promise = self.runtime().resolve(global);
                match self
                    .runtime()
                    .with_event_loop_promise(promise, PollEventLoopOptions::default())
                    .await
                {
                    Ok(global) => self.global_to_json(global)?,
                    Err(err) => {
                        let _ = watchdog_stop_tx.send(());
                        let _ = self.runtime().v8_isolate().cancel_terminate_execution();
                        if let Some(output) = self.recover_heap_exhaustion()? {
                            return Ok(output);
                        }
                        let stdout = self.take_stdout()?;
                        return self.shape_cell_output(
                            stdout,
                            None,
                            Some(self.terminated_error_or(err.to_string())),
                            shaped_output_limit,
                        );
                    }
                }
            }
            Err(err) => {
                let _ = watchdog_stop_tx.send(());
                let _ = self.runtime().v8_isolate().cancel_terminate_execution();
                if let Some(output) = self.recover_heap_exhaustion()? {
                    return Ok(output);
                }
                let error = self.terminated_error_or(err.to_string());
                let stdout = self.take_stdout()?;
                return self.shape_cell_output(stdout, None, Some(error), shaped_output_limit);
            }
        };
        let _ = watchdog_stop_tx.send(());
        let _ = self.runtime().v8_isolate().cancel_terminate_execution();

        let mut stdout = self.take_stdout()?;
        let displays = self.take_displays()?;
        for display in displays {
            let rendered = render_display(&display);
            if display.get("artifact").is_some() {
                push_line(&mut stdout, &format!("display {rendered}"));
                continue;
            }
            if display
                .get("opts")
                .and_then(|opts| opts.get("artifact"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let artifact = self
                    .artifact_store
                    .put_text(
                        redact_for_persistence(&rendered),
                        Some("display".to_string()),
                        "text/plain",
                    )
                    .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
                push_line(
                    &mut stdout,
                    &format!("display artifact: {} ({})", artifact.uri, artifact.preview),
                );
            } else {
                push_line(&mut stdout, &format!("display: {rendered}"));
            }
        }

        if let Some(value) = &result
            && !value.is_null()
        {
            let rendered = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
            if stdout.len().saturating_add(rendered.len()) > retained_output_limit {
                let artifact = match self.artifact_store.put_text(
                    redact_for_persistence(&rendered),
                    Some("cell result".to_string()),
                    "application/json",
                ) {
                    Ok(artifact) => artifact,
                    Err(_) => {
                        return Ok(EvalOutput {
                            stdout: String::new(),
                            result: None,
                            error: Some(format!(
                                "ResourceLimitError: cell output/result exceeded {} retained bytes",
                                self.options.limits.retained_output_bytes
                            )),
                        });
                    }
                };
                result = Some(json!({
                    "artifact": artifact.uri,
                    "preview": artifact.preview,
                    "sizeBytes": artifact.size_bytes,
                    "truncated": true
                }));
            }
        }
        self.shape_cell_output(stdout, result, None, shaped_output_limit)
    }

    async fn reset(&mut self) -> Result<()> {
        self.rebuild()
    }
}

impl DenoSession {
    fn rebuild(&mut self) -> Result<()> {
        let options = self.options.clone();
        self.runtime.take();
        *self = DenoSession::new(options)?;
        Ok(())
    }

    fn recover_heap_exhaustion(&mut self) -> Result<Option<EvalOutput>> {
        if !self.heap_exhausted.replace(false) {
            return Ok(None);
        }
        self.rebuild()?;
        Ok(Some(EvalOutput {
            stdout: String::new(),
            result: None,
            error: Some(
                "ResourceLimitError: V8 heap limit exceeded; sandbox isolate rebuilt".to_string(),
            ),
        }))
    }

    fn global_to_json(&mut self, global: v8::Global<v8::Value>) -> Result<Option<Value>> {
        let runtime = self.runtime();
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, global);
        Ok(serde_v8::from_v8::<Value>(scope, local).ok())
    }

    fn take_stdout(&mut self) -> Result<String> {
        let value = self
            .runtime()
            .execute_script(
                "<tempestmiku-stdout>",
                "globalThis.__tm_stdout.join('\\n') + \
                 (globalThis.__tm_output_truncated ? '\\n… output quota reached …' : '')",
            )
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let runtime = self.runtime();
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, value);
        serde_v8::from_v8::<String>(scope, local)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))
    }

    fn take_displays(&mut self) -> Result<Vec<Value>> {
        let value = self
            .runtime()
            .execute_script("<tempestmiku-displays>", "globalThis.__tm_displays")
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let runtime = self.runtime();
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, value);
        serde_v8::from_v8::<Vec<Value>>(scope, local)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))
    }

    fn cancelled(&self) -> bool {
        self.options
            .cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    fn terminated_error_or(&self, error: String) -> String {
        if error.contains("execution terminated") {
            if self.cancelled() {
                "CancellationError: cell cancelled".to_string()
            } else {
                "TimeoutError: cell exceeded wall-clock budget".to_string()
            }
        } else {
            error
        }
    }

    fn shape_cell_output(
        &self,
        stdout: String,
        result: Option<Value>,
        error: Option<String>,
        shape_limit: usize,
    ) -> Result<EvalOutput> {
        let rendered = render_eval_output(&stdout, result.as_ref(), error.as_deref());
        // Artifact references and structured quota markers are small control metadata, not
        // retained user output. Leave fixed headroom so a spill reference remains readable.
        let retained_with_metadata = self
            .options
            .limits
            .retained_output_bytes
            .saturating_add(4 * 1024);
        if rendered.len() > retained_with_metadata {
            return Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(format!(
                    "ResourceLimitError: cell output/result exceeded {} retained bytes",
                    self.options.limits.retained_output_bytes
                )),
            });
        }
        if rendered.len() <= shape_limit {
            return Ok(EvalOutput {
                stdout,
                result,
                error,
            });
        }
        let artifact = self
            .artifact_store
            .put_text(
                redact_for_persistence(&rendered),
                Some("cell output".to_string()),
                "text/plain",
            )
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let marker = format!(
            "cell output exceeded the {shape_limit}-byte result budget; full output at {}",
            artifact.uri
        );
        Ok(if error.is_some() {
            EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(marker),
            }
        } else {
            EvalOutput {
                stdout: marker,
                result: None,
                error: None,
            }
        })
    }
}

fn render_eval_output(stdout: &str, result: Option<&Value>, error: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(error) = error {
        parts.push(format!("error: {error}"));
    }
    if !stdout.is_empty() {
        parts.push(format!("stdout:\n{stdout}"));
    }
    if let Some(result) = result {
        let value = match result {
            Value::String(value) => value.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        };
        parts.push(format!("result:\n{value}"));
    }
    if parts.is_empty() {
        "(no output)".to_string()
    } else {
        parts.join("\n\n")
    }
}

fn redact_for_persistence(value: &str) -> String {
    tm_memory::redact_dream_text(value).text
}

fn push_line(out: &mut String, line: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
}

fn render_display(value: &Value) -> String {
    if let Some(artifact) = value.get("artifact") {
        let uri = artifact
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or("artifact://unknown");
        let preview = artifact
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return format!("artifact: {uri} ({preview})");
    }
    if let Some(rendered) = value.get("rendered").and_then(Value::as_str) {
        return rendered.to_string();
    }
    value
        .get("value")
        .map(|value| match value {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        })
        .unwrap_or_else(|| json!(null).to_string())
}
