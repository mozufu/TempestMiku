//! Sandbox backends.
//!
//! M0 keeps [`StubSandbox`] for protocol tests. M1 adds [`DenoSandbox`], a
//! `deno_core`-backed persistent JS/TS session with no ambient host I/O.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use async_trait::async_trait;
use deno_ast::{
    DecoratorsTranspileOption, EmitOptions, ImportsNotUsedAsValues, MediaType, ParseParams,
    SourceMapOption, TranspileModuleOptions, TranspileOptions, parse_script,
};
use deno_core::{
    JsRuntime, OpState, PollEventLoopOptions, RuntimeOptions, extension, op2, serde_v8, v8,
};
use deno_error::JsErrorBox;
use serde_json::{Value, json};
use tm_artifacts::{ArtifactRef, ArtifactStore, ResourceContent};
use tm_core::{CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};
use tm_host::{
    ArtifactResourceHandler, CapabilityGrants, HostError, HostFn, HostRegistry, InvocationCtx,
    ResourceRegistry,
};

/// A sandbox that runs no code. Each `eval` echoes the submitted source as its result and notes
/// the cell index in stdout, which is enough to validate the `tool_call -> tool_result -> final`
/// loop without a runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubSandbox;

#[async_trait]
impl Sandbox for StubSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(StubSession::default()))
    }
}

/// A persistent session for [`StubSandbox`].
#[derive(Debug, Default)]
pub struct StubSession {
    cells: usize,
}

#[async_trait(?Send)]
impl Session for StubSession {
    async fn eval(&mut self, code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        self.cells += 1;
        Ok(EvalOutput {
            stdout: format!(
                "[stub sandbox] no runtime yet (M1); echoing cell #{} ({} bytes)",
                self.cells,
                code.len()
            ),
            result: Some(Value::String(code.to_string())),
            error: None,
        })
    }

    async fn reset(&mut self) -> Result<()> {
        self.cells = 0;
        Ok(())
    }
}

#[derive(Clone)]
struct RuntimeHostState {
    artifact_store: ArtifactStore,
    host_registry: HostRegistry,
    resource_registry: ResourceRegistry,
    invocation_ctx: InvocationCtx,
}

#[derive(Debug, Clone)]
struct HttpGetFn {
    responses: BTreeMap<String, String>,
}

#[async_trait]
impl HostFn for HttpGetFn {
    fn name(&self) -> &str {
        "http.get"
    }

    async fn call(
        &self,
        args: Value,
        _ctx: &InvocationCtx,
    ) -> std::result::Result<Value, HostError> {
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| HostError::InvalidArgs("http.get requires a string url".to_string()))?;
        self.responses
            .get(url)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| HostError::CapabilityDenied("http.get".to_string()))
    }
}

#[op2]
#[serde]
async fn op_tm_host_call(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] args: serde_json::Value,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    host_state
        .host_registry
        .invoke(&name, args, &host_state.invocation_ctx)
        .await
        .map_err(js_host_error)
}

#[op2]
#[serde]
async fn op_tm_resource_read(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
    #[string] selector: String,
) -> std::result::Result<ResourceContent, JsErrorBox> {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    let selector = (!selector.is_empty()).then_some(selector);
    host_state
        .resource_registry
        .read(&uri, selector.as_deref(), &host_state.invocation_ctx)
        .await
        .map_err(js_host_error)
}

#[op2]
#[serde]
fn op_tm_artifact_put(
    state: &mut OpState,
    #[serde] data: serde_json::Value,
    #[serde] opts: serde_json::Value,
) -> std::result::Result<ArtifactRef, JsErrorBox> {
    let host_state = state.borrow::<RuntimeHostState>().clone();
    let title = opts
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mime = opts
        .get("mime")
        .and_then(Value::as_str)
        .unwrap_or("text/plain");
    let content = match data {
        Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).map_err(js_error)?,
    };
    host_state
        .artifact_store
        .put_text(content, title, mime)
        .map_err(js_error)
}

#[op2]
#[serde]
fn op_tm_artifact_list(state: &mut OpState) -> Vec<ArtifactRef> {
    state.borrow::<RuntimeHostState>().artifact_store.list()
}

fn js_host_error(err: HostError) -> JsErrorBox {
    let name = match &err {
        HostError::CapabilityDenied(_) => "CapabilityDeniedError",
        HostError::ApprovalDenied(_) => "ApprovalDeniedError",
        HostError::ApprovalTimeout(_) => "ApprovalTimeoutError",
        HostError::UnknownScheme { .. } => "CapabilityDeniedError",
        HostError::NotFound(_) => "NotFoundError",
        HostError::InvalidArgs(_) => "InvalidArgsError",
        HostError::HostCall(_) => "HostCallError",
    };
    JsErrorBox::generic(format!("{name}: {err}"))
}

fn js_error(err: impl ToString) -> JsErrorBox {
    JsErrorBox::generic(err.to_string())
}

extension!(
    tm_sandbox_ops,
    ops = [
        op_tm_host_call,
        op_tm_resource_read,
        op_tm_artifact_put,
        op_tm_artifact_list
    ],
    options = {
        host_state: RuntimeHostState,
    },
    state = |state, options| {
        state.put(options.host_state);
    },
);

/// Configuration for [`DenoSandbox`].
#[derive(Clone)]
pub struct DenoSandboxOptions {
    pub artifact_root: PathBuf,
    pub session_id: String,
    pub http_allowlist: BTreeMap<String, String>,
    pub host_registry: HostRegistry,
    pub resource_registry: ResourceRegistry,
    pub grants: CapabilityGrants,
}

impl Default for DenoSandboxOptions {
    fn default() -> Self {
        Self {
            artifact_root: tm_artifacts::default_root(),
            session_id: "default".to_string(),
            http_allowlist: BTreeMap::new(),
            host_registry: HostRegistry::new(),
            resource_registry: ResourceRegistry::new(),
            grants: CapabilityGrants::default()
                .allow("http.get")
                .allow("resources.read:artifact"),
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
        host_registry.register(Arc::new(HttpGetFn {
            responses: options.http_allowlist.clone(),
        }));
        let mut resource_registry = options.resource_registry.clone();
        resource_registry.register(Arc::new(ArtifactResourceHandler::new(
            artifact_store.clone(),
        )));
        let host_state = RuntimeHostState {
            artifact_store: artifact_store.clone(),
            host_registry,
            resource_registry,
            invocation_ctx: InvocationCtx::new(options.grants.clone()),
        };
        let mut session = Self {
            runtime: Some(JsRuntime::new(RuntimeOptions {
                extensions: vec![tm_sandbox_ops::init(host_state)],
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
        let prelude = r#"
const __tm_ops = globalThis.Deno?.core?.ops;
if (!__tm_ops) throw new Error("HostCallError: Deno core ops unavailable");
try {
  Object.defineProperty(globalThis, "Deno", { value: undefined, writable: true, configurable: true });
} catch (_) {
  try { globalThis.Deno = undefined; } catch (_) {}
}
globalThis.fetch = undefined;
globalThis.__tm_stdout = [];
globalThis.__tm_displays = [];
globalThis.print = (...items) => {
  globalThis.__tm_stdout.push(items.map((item) =>
    typeof item === "string" ? item : JSON.stringify(item)
  ).join(" "));
};
globalThis.display = (value, opts = undefined) => {
  globalThis.__tm_displays.push({ value, opts });
};
const __tm_uri = (ref) => typeof ref === "string" ? ref : ref.uri;
const __tm_selector = (opts) => {
  const selector = opts && typeof opts === "object" ? opts.selector : undefined;
  return selector == null ? "" : String(selector);
};
const __tm_arg_selector = (selector) => selector == null ? "" : String(selector);
const __tm_sdk_shape = (value) => {
  if (!value || typeof value !== "object") return value;
  const shaped = { ...value };
  if (Object.prototype.hasOwnProperty.call(shaped, "size_bytes")) {
    shaped.sizeBytes = shaped.size_bytes;
    delete shaped.size_bytes;
  }
  if (Object.prototype.hasOwnProperty.call(shaped, "has_more")) {
    shaped.hasMore = shaped.has_more;
    delete shaped.has_more;
  }
  return shaped;
};
globalThis.artifacts = {
  put: (data, opts = undefined) => __tm_sdk_shape(__tm_ops.op_tm_artifact_put(data, opts ?? null)),
  get: async (ref, opts = undefined) => __tm_sdk_shape(await __tm_ops.op_tm_resource_read(__tm_uri(ref), __tm_selector(opts))),
  slice: async (ref, selector) => artifacts.get(ref, { selector }),
  list: () => __tm_ops.op_tm_artifact_list().map(__tm_sdk_shape)
};
globalThis.resources = {
  read: async (uri, selector = undefined) => __tm_sdk_shape(await __tm_ops.op_tm_resource_read(String(uri), __tm_arg_selector(selector))),
  preview: async (uri) => resources.read(uri),
  list: (_uri = undefined) => artifacts.list()
};
globalThis.tools = {
  search: async () => [],
  docs: async (name) => tools.call("tools.docs", { name: String(name) }),
  call: async (name, args = {}) => __tm_ops.op_tm_host_call(String(name), args ?? null)
};
globalThis.http = {
  get: async (url) => tools.call("http.get", { url: String(url) })
};
globalThis.secrets = undefined;
globalThis.memory = undefined;
globalThis.skills = undefined;
globalThis.agents = undefined;
"#;
        self.runtime()
            .execute_script("<tempestmiku-prelude>", prelude)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl Session for DenoSession {
    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput> {
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
        let (timeout_cancel_tx, timeout_cancel_rx) = mpsc::channel();
        let isolate_handle = self.runtime().v8_isolate().thread_safe_handle();
        let wall_ms = budget.wall_ms;
        thread::spawn(move || {
            if timeout_cancel_rx
                .recv_timeout(Duration::from_millis(wall_ms))
                .is_err()
            {
                isolate_handle.terminate_execution();
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
                            let _ = timeout_cancel_tx.send(());
                            let _ = self.runtime().v8_isolate().cancel_terminate_execution();
                            return Ok(EvalOutput {
                                stdout: self.take_stdout()?,
                                result: None,
                                error: Some(err.to_string()),
                            });
                        }
                    }
                } else {
                    self.global_to_json(global)?
                }
            }
            Err(err) => {
                let _ = timeout_cancel_tx.send(());
                let _ = self.runtime().v8_isolate().cancel_terminate_execution();
                let error = if err.to_string().contains("execution terminated") {
                    "TimeoutError: cell exceeded wall-clock budget".to_string()
                } else {
                    err.to_string()
                };
                return Ok(EvalOutput {
                    stdout: self.take_stdout()?,
                    result: None,
                    error: Some(error),
                });
            }
        };
        let _ = timeout_cancel_tx.send(());
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

fn starts_with_top_level_await(code: &str) -> bool {
    code.contains("await ")
}

fn lower_top_level_await(code: &str) -> String {
    if code.contains("await ") {
        wrap_async_cell(code)
    } else {
        code.to_string()
    }
}

fn wrap_async_cell(code: &str) -> String {
    let trimmed = code.trim();
    if !trimmed.contains(';') {
        return format!("(async () => await ({trimmed}))()");
    }
    let mut parts = trimmed.rsplitn(2, ';');
    let tail = parts.next().unwrap_or("").trim();
    let head = parts.next().unwrap_or("").trim_end();
    if tail.is_empty() {
        format!("(async () => {{\n{trimmed}\n}})()")
    } else {
        format!("(async () => {{\n{head};\nreturn ({tail});\n}})()")
    }
}

fn transpile_typescript(code: &str) -> Result<String> {
    let specifier = deno_ast::ModuleSpecifier::parse("file:///cell.ts")
        .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
    let parsed = parse_script(ParseParams {
        specifier,
        text: code.into(),
        media_type: MediaType::TypeScript,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
    let transpiled = parsed
        .transpile(
            &TranspileOptions {
                imports_not_used_as_values: ImportsNotUsedAsValues::Remove,
                decorators: DecoratorsTranspileOption::Ecma,
                ..TranspileOptions::default()
            },
            &TranspileModuleOptions::default(),
            &EmitOptions {
                source_map: SourceMapOption::None,
                ..EmitOptions::default()
            },
        )
        .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
    Ok(transpiled.into_source().text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_echoes_code_and_persists_cell_count() {
        let sandbox = StubSandbox;
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

        let out = session.eval("1 + 1", CellBudget::default()).await.unwrap();
        assert_eq!(out.result, Some(Value::String("1 + 1".into())));
        assert!(out.stdout.contains("cell #1"));

        let out2 = session.eval("2 + 2", CellBudget::default()).await.unwrap();
        assert!(out2.stdout.contains("cell #2"));

        session.reset().await.unwrap();
        let out3 = session.eval("3", CellBudget::default()).await.unwrap();
        assert!(out3.stdout.contains("cell #1"));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_executes_typescript_cell() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "interface Box<T> { value: T }\n\
                 type Label = string;\n\
                 const box: Box<number> = { value: 41 };\n\
                 const label = 'x' as Label;\n\
                 box.value + label.length",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.result, Some(Value::Number(42.into())));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_parse_errors_are_cell_errors() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval("const broken: = ;", CellBudget::default())
            .await
            .unwrap();
        assert!(out.error.is_some());

        let after = session.eval("1 + 1", CellBudget::default()).await.unwrap();
        assert_eq!(after.result, Some(Value::Number(2.into())));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_executes_multiline_cells() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "const x: number = 1;\nconst y: number = 2;\nx + y",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.result, Some(Value::Number(3.into())));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_persists_state_and_resets() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        session
            .eval(
                "let count: number = 1;\n\
                 function add_one(n: number): number { return n + 1; }\n\
                 0",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let out = session
            .eval("add_one(count)", CellBudget::default())
            .await
            .unwrap();
        assert_eq!(out.result, Some(Value::Number(2.into())));
        session.reset().await.unwrap();
        let out = session
            .eval("add_one(1)", CellBudget::default())
            .await
            .unwrap();
        assert!(out.error.is_some());
        let out = session.eval("count", CellBudget::default()).await.unwrap();
        assert!(out.error.is_some());
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_timeout_is_structured_error() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "while (true) {}",
                CellBudget {
                    wall_ms: 10,
                    ..CellBudget::default()
                },
            )
            .await
            .unwrap();
        assert!(out.error.unwrap().contains("TimeoutError"));
        let after = session.eval("1 + 1", CellBudget::default()).await.unwrap();
        assert_eq!(after.result, Some(Value::Number(2.into())));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_captures_print_and_display() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "print('hello', 1); display({ ok: true }); 7",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert!(out.stdout.contains("hello 1"));
        assert!(out.stdout.contains("display"));
        assert_eq!(out.result, Some(Value::Number(7.into())));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_blocks_ambient_raw_apis() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "({ deno: typeof Deno, fetch: typeof fetch, process: typeof process })",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let result = out.result.unwrap();
        assert_eq!(result["deno"], Value::String("undefined".into()));
        assert_eq!(result["fetch"], Value::String("undefined".into()));
        assert_eq!(result["process"], Value::String("undefined".into()));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_spills_large_output_to_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = DenoSandbox::new(DenoSandboxOptions {
            artifact_root: dir.path().to_path_buf(),
            ..DenoSandboxOptions::default()
        });
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "print('x'.repeat(100));",
                CellBudget {
                    output_bytes: 20,
                    ..CellBudget::default()
                },
            )
            .await
            .unwrap();
        assert!(out.stdout.contains("artifact://"));
        assert!(out.stdout.contains("output truncated to 20 bytes"));
        assert!(!out.stdout.contains(&"x".repeat(100)));
        let fetched = session
            .eval(
                "const first = artifacts.list()[0].uri; await artifacts.get(first)",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            fetched.result.unwrap()["content"].as_str().unwrap().len(),
            100
        );
        let listed = session
            .eval("artifacts.list()[0].sizeBytes", CellBudget::default())
            .await
            .unwrap();
        assert_eq!(listed.result, Some(Value::Number(100.into())));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_artifacts_resolve_through_resource_registry() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = DenoSandbox::new(DenoSandboxOptions {
            artifact_root: dir.path().to_path_buf(),
            ..DenoSandboxOptions::default()
        });
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "const ref = artifacts.put('one\\ntwo', { title: 'manual' });\n\
                 await resources.read(ref.uri, '2-2')",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let result = out.result.unwrap();
        assert_eq!(result["content"], Value::String("two".into()));
        assert_eq!(result["sizeBytes"], Value::Number(7.into()));
        assert_eq!(result["hasMore"], Value::Bool(false));

        let denied = session
            .eval("await resources.read('memory://x')", CellBudget::default())
            .await
            .unwrap();
        let error = denied.error.unwrap();
        assert!(error.contains("CapabilityDeniedError"));
        assert!(error.contains("unknown resource scheme"));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_unknown_host_capability_fails_closed() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "await tools.call('missing.capability', {})",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let error = out.error.unwrap();
        assert!(error.contains("CapabilityDeniedError"));
        assert!(error.contains("missing.capability"));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_http_get_is_default_deny_and_allowlisted() {
        let mut http_allowlist = BTreeMap::new();
        http_allowlist.insert("https://local.test/ok".to_string(), "ok".to_string());
        let sandbox = DenoSandbox::new(DenoSandboxOptions {
            http_allowlist,
            ..DenoSandboxOptions::default()
        });
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let denied = session
            .eval(
                "await http.get('https://evil.test/')",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert!(denied.error.unwrap().contains("CapabilityDeniedError"));
        let allowed = session
            .eval(
                "await http.get('https://local.test/ok')",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.result, Some(Value::String("ok".into())));
        let composed = session
            .eval(
                "const body = await http.get('https://local.test/ok'); display(body)",
                CellBudget::default(),
            )
            .await
            .unwrap();
        assert!(composed.stdout.contains("display: ok"));
    }
}
