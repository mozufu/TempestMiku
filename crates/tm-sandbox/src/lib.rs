//! Sandbox backends.
//!
//! M0 keeps [`StubSandbox`] for protocol tests. M1 adds [`DenoSandbox`], a
//! `deno_core`-backed persistent JS/TS session with no ambient host I/O.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

use async_trait::async_trait;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions, serde_v8, v8};
use serde_json::{Value, json};
use tm_artifacts::ArtifactStore;

use tm_core::{CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};

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

/// Configuration for [`DenoSandbox`].
#[derive(Debug, Clone)]
pub struct DenoSandboxOptions {
    pub artifact_root: PathBuf,
    pub session_id: String,
    pub http_allowlist: BTreeMap<String, String>,
}

impl Default for DenoSandboxOptions {
    fn default() -> Self {
        Self {
            artifact_root: tm_artifacts::default_root(),
            session_id: "default".to_string(),
            http_allowlist: BTreeMap::new(),
        }
    }
}

/// A `deno_core`-backed persistent JavaScript/TypeScript sandbox.
#[derive(Debug, Clone, Default)]
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
    runtime: JsRuntime,
    artifact_store: ArtifactStore,
    http_allowlist: BTreeMap<String, String>,
    globals: BTreeMap<String, Value>,
    cleared_globals: BTreeSet<String>,
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
        let mut session = Self {
            runtime: JsRuntime::new(RuntimeOptions::default()),
            artifact_store,
            globals: BTreeMap::new(),
            cleared_globals: BTreeSet::new(),
            http_allowlist: options.http_allowlist,
        };
        session.install_prelude()?;
        Ok(session)
    }

    fn install_prelude(&mut self) -> Result<()> {
        let http_map = serde_json::to_string(&self.http_allowlist)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let prelude = format!(
            r#"
globalThis.__tm_stdout = [];
globalThis.__tm_displays = [];
globalThis.__tm_http_allowlist = {http_map};
globalThis.__tm_artifacts = globalThis.__tm_artifacts ?? {{}};
globalThis.print = (...items) => {{
  globalThis.__tm_stdout.push(items.map((item) =>
    typeof item === "string" ? item : JSON.stringify(item)
  ).join(" "));
}};
globalThis.display = (value, opts = undefined) => {{
  globalThis.__tm_displays.push({{ value, opts }});
}};
globalThis.artifacts = {{
  get: async (ref, opts = undefined) => {{
    const uri = typeof ref === "string" ? ref : ref.uri;
    const entry = globalThis.__tm_artifacts[uri];
    if (!entry) throw new Error(`NotFoundError: ${{uri}}`);
    const selector = opts?.selector;
    let content = entry;
    let hasMore = false;
    if (selector) {{
      const match = String(selector).match(/^(\\d+)-(\\d+)$/);
      if (!match) throw new Error(`InvalidArgsError: invalid selector ${{selector}}`);
      const start = Number(match[1]);
      const end = Number(match[2]);
      const lines = entry.split("\\n");
      content = lines.slice(start - 1, end).join("\\n");
      hasMore = end < lines.length;
    }}
    return {{ uri, kind: "text", mime: "text/plain", sizeBytes: entry.length, content, preview: entry.slice(0, 1024), selector, hasMore }};
  }},
  slice: async (ref, selector) => artifacts.get(ref, {{ selector }}),
  list: () => Object.keys(globalThis.__tm_artifacts).map((uri) => ({{ uri, id: uri.replace("artifact://", ""), kind: "text", mime: "text/plain", sizeBytes: globalThis.__tm_artifacts[uri].length, preview: globalThis.__tm_artifacts[uri].slice(0, 1024) }}))
}};
globalThis.resources = {{
  read: async (uri, selector = undefined) => {{
    if (!uri.startsWith("artifact://")) throw new Error(`CapabilityDeniedError: unknown resource scheme for ${{uri}}`);
    return artifacts.get(uri, {{ selector }});
  }},
  preview: async (uri) => resources.read(uri),
  list: (_uri = undefined) => artifacts.list()
}};
globalThis.tools = {{
  search: async () => [],
  docs: async () => {{ throw new Error("NotImplementedError"); }},
  call: async () => {{ throw new Error("CapabilityDeniedError"); }}
}};
globalThis.http = {{
  get: async (url) => {{
    if (!Object.prototype.hasOwnProperty.call(globalThis.__tm_http_allowlist, url)) {{
      throw new Error("CapabilityDeniedError: http.get default-deny");
    }}
    return globalThis.__tm_http_allowlist[url];
  }}
}};
globalThis.secrets = undefined;
globalThis.memory = undefined;
globalThis.skills = undefined;
globalThis.agents = undefined;
"#
        );
        self.runtime
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

        if is_cleared_identifier(&self.cleared_globals, code) {
            return Ok(EvalOutput {
                stdout: String::new(),
                result: None,
                error: Some(format!("ReferenceError: {} is not defined", code.trim())),
            });
        }
        self.runtime
            .execute_script(
                "<tempestmiku-clear>",
                "globalThis.__tm_stdout = []; globalThis.__tm_displays = [];",
            )
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;

        let code = strip_typescript(code);
        let code_for_state = code.clone();
        let wants_await = starts_with_top_level_await(&code);
        let code = lower_top_level_await(&code);
        let (timeout_cancel_tx, timeout_cancel_rx) = mpsc::channel();
        let isolate_handle = self.runtime.v8_isolate().thread_safe_handle();
        let wall_ms = budget.wall_ms;
        thread::spawn(move || {
            if timeout_cancel_rx
                .recv_timeout(Duration::from_millis(wall_ms))
                .is_err()
            {
                isolate_handle.terminate_execution();
            }
        });

        let mut result = match self.runtime.execute_script("<cell>", code) {
            Ok(global) => {
                if wants_await {
                    let promise = self.runtime.resolve(global);
                    match self
                        .runtime
                        .with_event_loop_promise(promise, PollEventLoopOptions::default())
                        .await
                    {
                        Ok(global) => self.global_to_json(global)?,
                        Err(err) => {
                            let _ = timeout_cancel_tx.send(());
                            let _ = self.runtime.v8_isolate().cancel_terminate_execution();
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
                let _ = self.runtime.v8_isolate().cancel_terminate_execution();
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
        let _ = self.runtime.v8_isolate().cancel_terminate_execution();

        let mut stdout = self.take_stdout()?;
        let displays = self.take_displays()?;
        remember_globals(&mut self.globals, &code_for_state);
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
                self.register_artifact_js(&artifact.uri, &rendered)?;
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
                self.register_artifact_js(&artifact.uri, &rendered)?;
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
            self.register_artifact_js(&artifact.uri, &stdout)?;
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
        self.cleared_globals.extend(self.globals.keys().cloned());
        for name in self.globals.keys() {
            let script = format!("delete globalThis[{name:?}];");
            self.runtime
                .execute_script("<tempestmiku-reset>", script)
                .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        }
        self.globals.clear();
        Ok(())
    }
}

impl DenoSession {
    fn register_artifact_js(&mut self, uri: &str, content: &str) -> Result<()> {
        let uri =
            serde_json::to_string(uri).map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let content = serde_json::to_string(content)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        let script = format!("globalThis.__tm_artifacts[{uri}] = {content};");
        self.runtime
            .execute_script("<tempestmiku-artifact-register>", script)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        Ok(())
    }

    fn global_to_json(&mut self, global: v8::Global<v8::Value>) -> Result<Option<Value>> {
        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, global);
        Ok(serde_v8::from_v8::<Value>(scope, local).ok())
    }

    fn take_stdout(&mut self) -> Result<String> {
        let value = self
            .runtime
            .execute_script("<tempestmiku-stdout>", "globalThis.__tm_stdout.join('\\n')")
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, value);
        serde_v8::from_v8::<String>(scope, local)
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))
    }

    fn take_displays(&mut self) -> Result<Vec<Value>> {
        let value = self
            .runtime
            .execute_script("<tempestmiku-displays>", "globalThis.__tm_displays")
            .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
        deno_core::scope!(scope, &mut self.runtime);
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

fn is_cleared_identifier(cleared_globals: &BTreeSet<String>, code: &str) -> bool {
    let name = code.trim();
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && cleared_globals.contains(name)
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

fn strip_typescript(code: &str) -> String {
    code.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("interface ") && !trimmed.starts_with("type ")
        })
        .map(strip_type_annotations)
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_type_annotations(line: &str) -> String {
    let trimmed = line.trim_start();
    let Some(keyword) = ["const ", "let ", "var "]
        .iter()
        .find(|keyword| trimmed.starts_with(**keyword))
    else {
        return line.to_string();
    };
    let indent = &line[..line.len() - trimmed.len()];
    let rest = &trimmed[keyword.len()..];
    let Some((name_part, value)) = rest.split_once('=') else {
        return line.to_string();
    };
    let name = name_part.split(':').next().unwrap_or(name_part).trim();
    if !name
        .chars()
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return line.to_string();
    }
    format!("{indent}globalThis.{name} = {}", value.trim_start())
}

fn remember_globals(globals: &mut BTreeMap<String, Value>, code: &str) {
    for statement in code.split(';') {
        let statement = statement.trim();
        let rest = statement.strip_prefix("globalThis.").or_else(|| {
            statement
                .strip_prefix("const ")
                .or_else(|| statement.strip_prefix("let "))
                .or_else(|| statement.strip_prefix("var "))
        });
        let Some(rest) = rest else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.split(':').next().unwrap_or(name).trim();
        if !name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        {
            continue;
        }
        let value = value.trim();
        let parsed = serde_json::from_str::<Value>(value)
            .ok()
            .or_else(|| value.parse::<i64>().ok().map(|n| Value::Number(n.into())))
            .or_else(|| match value {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ if value.starts_with('"') && value.ends_with('"') => {
                    Some(Value::String(value.trim_matches('"').to_string()))
                }
                _ if value.starts_with('\'') && value.ends_with('\'') => {
                    Some(Value::String(value.trim_matches('\'').to_string()))
                }
                _ => None,
            });
        if let Some(value) = parsed {
            globals.insert(name.to_string(), value);
        }
    }
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
            .eval("const x: number = 41; x + 1", CellBudget::default())
            .await
            .unwrap();
        assert_eq!(out.result, Some(Value::Number(42.into())));
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
            .eval("let count = 1; 0", CellBudget::default())
            .await
            .unwrap();
        let out = session
            .eval("count + 1", CellBudget::default())
            .await
            .unwrap();
        assert_eq!(out.result, Some(Value::Number(2.into())));
        session.reset().await.unwrap();
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
