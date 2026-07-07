use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions, serde_v8, v8};
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;
use tm_core::{CancellationToken, CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};
use tm_host::{
    ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants, DefaultDenyApprovalPolicy,
    HostRegistry, InvocationCtx, LinkedFolders, ResourceRegistry,
    register_p0_linked_folder_functions,
};

use crate::{
    ops::{HttpGetFn, RuntimeHostState, init_ops},
    prelude::{AGENTS_PRELUDE, SDK_PRELUDE},
    ts::{lower_top_level_await, starts_with_top_level_await, transpile_typescript},
};

/// Configuration for [`DenoSandbox`].
#[derive(Clone)]
pub struct DenoSandboxOptions {
    pub artifact_root: PathBuf,
    pub session_id: String,
    pub actor_id: Option<String>,
    pub http_allowlist: BTreeMap<String, String>,
    pub host_registry: HostRegistry,
    pub resource_registry: ResourceRegistry,
    pub grants: CapabilityGrants,
    pub linked_folders: Option<LinkedFolders>,
    pub approval_policy: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
    pub cancellation: Option<Arc<dyn CancellationToken>>,
}

impl Default for DenoSandboxOptions {
    fn default() -> Self {
        Self {
            artifact_root: tm_artifacts::default_root(),
            session_id: "default".to_string(),
            actor_id: None,
            http_allowlist: BTreeMap::new(),
            host_registry: HostRegistry::new(),
            resource_registry: ResourceRegistry::new(),
            grants: CapabilityGrants::default()
                .allow("http.get")
                .allow("resources.read:artifact"),
            linked_folders: None,
            approval_policy: Arc::new(DefaultDenyApprovalPolicy),
            approval_timeout: Duration::from_secs(60),
            cancellation: None,
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
}

// `JsRuntime` is single-thread-affine in practice. TempestMiku sessions are
// owned behind `&mut dyn Session`; callers must not evaluate one session
// concurrently. The core trait requires `Session: Send` so boxed sessions can
// cross task boundaries between cells.
unsafe impl Send for DenoSession {}

impl DenoSession {
    fn new(options: DenoSandboxOptions) -> Result<Self> {
        let artifact_store = ArtifactStore::open(&options.artifact_root, &options.session_id)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let mut host_registry = options.host_registry.clone();
        host_registry.register(Arc::new(HttpGetFn::new(options.http_allowlist.clone())));
        let mut resource_registry = options.resource_registry.clone();
        resource_registry.register(Arc::new(ArtifactResourceHandler::new(
            artifact_store.clone(),
        )));
        let mut grants = options.grants.clone();
        if let Some(linked_folders) = options.linked_folders.clone() {
            register_p0_linked_folder_functions(
                &mut host_registry,
                &mut resource_registry,
                linked_folders,
                artifact_store.clone(),
            );
            grants = grants.allow_many([
                "fs.read",
                "fs.write",
                "fs.ls",
                "fs.find",
                "code.search",
                "code.edit",
                "proc.run",
                "resources.read:linked",
            ]);
        }
        let host_state = RuntimeHostState {
            artifact_store: artifact_store.clone(),
            host_registry,
            resource_registry,
            invocation_ctx: InvocationCtx::with_approvals(
                grants,
                options.approval_policy.clone(),
                options.approval_timeout,
            )
            .with_session_id(options.session_id.clone())
            .with_actor_id(options.actor_id.clone()),
        };
        let mut session = Self {
            runtime: Some(JsRuntime::new(RuntimeOptions {
                extensions: vec![init_ops(host_state)],
                ..RuntimeOptions::default()
            })),
            artifact_store,
            options,
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

        self.runtime()
            .execute_script(
                "<tempestmiku-clear>",
                "globalThis.__tm_stdout = []; globalThis.__tm_displays = [];",
            )
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;

        let wants_await = starts_with_top_level_await(code);
        let code = lower_top_level_await(code);
        let code = match transpile_typescript(&code) {
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
                if wants_await {
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
                            return Ok(EvalOutput {
                                stdout: self.take_stdout()?,
                                result: None,
                                error: Some(self.terminated_error_or(err.to_string())),
                            });
                        }
                    }
                } else {
                    self.global_to_json(global)?
                }
            }
            Err(err) => {
                let _ = watchdog_stop_tx.send(());
                let _ = self.runtime().v8_isolate().cancel_terminate_execution();
                let error = self.terminated_error_or(err.to_string());
                return Ok(EvalOutput {
                    stdout: self.take_stdout()?,
                    result: None,
                    error: Some(error),
                });
            }
        };
        let _ = watchdog_stop_tx.send(());
        let _ = self.runtime().v8_isolate().cancel_terminate_execution();

        let mut stdout = self.take_stdout()?;
        let displays = self.take_displays()?;
        for display in displays {
            let rendered = render_display(&display);
            if display
                .get("opts")
                .and_then(|opts| opts.get("artifact"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let artifact = self
                    .artifact_store
                    .put_text(&rendered, Some("display".to_string()), "text/plain")
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
            if stdout.len().saturating_add(rendered.len()) > budget.output_bytes {
                let artifact = self
                    .artifact_store
                    .put_text(
                        &rendered,
                        Some("cell result".to_string()),
                        "application/json",
                    )
                    .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
                result = Some(json!({
                    "artifact": artifact.uri,
                    "preview": artifact.preview,
                    "sizeBytes": artifact.size_bytes,
                    "truncated": true
                }));
            }
        }

        if stdout.len() > budget.output_bytes {
            let artifact = self
                .artifact_store
                .put_text(&stdout, Some("cell stdout".to_string()), "text/plain")
                .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
            stdout = format!(
                "{}\n… output truncated to {} bytes; full output at {}",
                tm_artifacts::preview(&stdout, budget.output_bytes),
                budget.output_bytes,
                artifact.uri
            );
        }

        Ok(EvalOutput {
            stdout,
            result,
            error: None,
        })
    }

    async fn reset(&mut self) -> Result<()> {
        let options = self.options.clone();
        self.runtime.take();
        *self = DenoSession::new(options)?;
        Ok(())
    }
}

impl DenoSession {
    fn global_to_json(&mut self, global: v8::Global<v8::Value>) -> Result<Option<Value>> {
        let runtime = self.runtime();
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, global);
        Ok(serde_v8::from_v8::<Value>(scope, local).ok())
    }

    fn take_stdout(&mut self) -> Result<String> {
        let value = self
            .runtime()
            .execute_script("<tempestmiku-stdout>", "globalThis.__tm_stdout.join('\\n')")
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
}

fn push_line(out: &mut String, line: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
}

fn render_display(value: &Value) -> String {
    value
        .get("value")
        .map(|value| match value {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        })
        .unwrap_or_else(|| json!(null).to_string())
}
