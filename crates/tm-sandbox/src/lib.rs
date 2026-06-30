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
use serde::Serialize;
use serde_json::{Value, json};
use tm_artifacts::{ArtifactRef, ArtifactStore};
use tm_core::{CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};
use tm_host::{
    ApprovalPolicy, ArtifactResourceHandler, CapabilityGrants, DefaultDenyApprovalPolicy, GrantDoc,
    HostError, HostFn, HostRegistry, InvocationCtx, LinkedFolders, ResourceRegistry, ToolDocs,
    ToolErrorDoc, ToolExample, ToolSummary, register_p0_linked_folder_functions,
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
    docs: ToolDocs,
}

impl HttpGetFn {
    fn new(responses: BTreeMap<String, String>) -> Self {
        Self {
            responses,
            docs: ToolDocs {
                name: "http.get".to_string(),
                namespace: "http".to_string(),
                summary: "Fetch a deterministic allowlisted HTTP response".to_string(),
                description: Some(
                    "M1/P0 exposes http.get as a default-deny, deterministic allowlist helper. It is not ambient network egress; production egress policy remains deferred."
                        .to_string(),
                ),
                signature: "http.get(url: string): Promise<string>".to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["url"],
                    "additionalProperties": false,
                    "properties": {
                        "url": {
                            "type": "string",
                            "format": "uri",
                            "description": "URL must be present in the session's deterministic allowlist."
                        }
                    }
                }),
                result_schema: Some(json!({ "type": "string" })),
                examples: vec![ToolExample {
                    title: Some("Fetch allowlisted fixture".to_string()),
                    code: "const body = await http.get('https://local.test/ok');\ndisplay(body);"
                        .to_string(),
                    notes: Some("Non-allowlisted URLs fail closed with CapabilityDeniedError.".to_string()),
                }],
                errors: vec![
                    ToolErrorDoc {
                        name: "CapabilityDeniedError".to_string(),
                        when: "The URL is not in the session allowlist or http.get is not granted."
                            .to_string(),
                        retryable: false,
                    },
                    ToolErrorDoc {
                        name: "InvalidArgsError".to_string(),
                        when: "The url argument is missing or not a string.".to_string(),
                        retryable: false,
                    },
                ],
                grants: vec![GrantDoc {
                    kind: "network".to_string(),
                    description:
                        "Deterministic allowlisted HTTP fixture access; no open egress.".to_string(),
                }],
                sensitive: true,
                approval: "none".to_string(),
                since: "M1".to_string(),
                stability: "experimental".to_string(),
            },
        }
    }
}

#[async_trait]
impl HostFn for HttpGetFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
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

fn core_tool_docs() -> BTreeMap<String, ToolDocs> {
    [
        core_doc(
            "resources.read",
            "resources",
            "Read a registered resource URI",
            "resources.read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>",
            "Read an artifact, linked file, or future resource URI through the scheme-dispatched resource registry. Scheme-specific grants still apply.",
            json!({
                "type": "object",
                "required": ["uri"],
                "additionalProperties": false,
                "properties": {
                    "uri": { "type": "string" },
                    "selector": { "type": "string", "description": "Optional resource selector such as a 1-based line range." }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Read artifact lines".to_string()),
                code: "const ref = artifacts.put('one\\ntwo');\nconst content = await resources.read(ref.uri, '2-2');\ndisplay(content.content);".to_string(),
                notes: Some("Unknown schemes and missing scheme grants fail closed with CapabilityDeniedError.".to_string()),
            }],
            resource_errors("resources.read"),
            vec![GrantDoc {
                kind: "workspace".to_string(),
                description: "Scheme-specific grants such as resources.read:artifact or resources.read:linked.".to_string(),
            }],
        ),
        core_doc(
            "resources.preview",
            "resources",
            "Preview a registered resource URI",
            "resources.preview(uri: ResourceUri): Promise<ResourceContent>",
            "Return a ResourceContent envelope with preview metadata for a registered resource URI.",
            json!({
                "type": "object",
                "required": ["uri"],
                "additionalProperties": false,
                "properties": {
                    "uri": { "type": "string" }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Preview an artifact".to_string()),
                code: "const ref = artifacts.put('long text');\nconst preview = await resources.preview(ref.uri);".to_string(),
                notes: None,
            }],
            resource_errors("resources.preview"),
            vec![GrantDoc {
                kind: "workspace".to_string(),
                description: "Scheme-specific grants such as resources.read:artifact or resources.read:linked.".to_string(),
            }],
        ),
        core_doc(
            "resources.list",
            "resources",
            "List registered resource schemes or entries",
            "resources.list(uri?: ResourceUri): Promise<ResourceEntry[]>",
            "List registered resource schemes, or entries beneath a URI when that scheme supports listing.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "uri": { "type": "string", "description": "Omit to list registered schemes." }
                }
            }),
            Some(json!({ "type": "array", "items": resource_entry_schema() })),
            vec![ToolExample {
                title: Some("List schemes".to_string()),
                code: "const schemes = await resources.list();\ndisplay(schemes, { kind: 'json' });".to_string(),
                notes: None,
            }],
            resource_errors("resources.list"),
            vec![GrantDoc {
                kind: "workspace".to_string(),
                description: "Listing a specific URI uses that scheme's resource grant.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.put",
            "artifacts",
            "Store session-local text or JSON",
            "artifacts.put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef",
            "Store a session-local text or JSON artifact and return an artifact:// handle. P0 artifacts are text-backed.",
            json!({
                "type": "object",
                "required": ["data"],
                "additionalProperties": false,
                "properties": {
                    "data": {},
                    "title": { "type": "string" },
                    "mime": { "type": "string", "default": "text/plain" },
                    "kind": { "type": "string", "description": "Reserved metadata hint in P0." },
                    "filename": { "type": "string", "description": "Reserved metadata hint in P0." }
                }
            }),
            Some(artifact_ref_schema()),
            vec![ToolExample {
                title: Some("Create an artifact".to_string()),
                code: "const ref = artifacts.put('notes\\n', { title: 'notes' });\ndisplay(ref.uri);".to_string(),
                notes: None,
            }],
            vec![tool_error("HostCallError", "The artifact store cannot write the artifact.", false)],
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Session-local artifact writes are always available inside the sandbox.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.get",
            "artifacts",
            "Read a session artifact",
            "artifacts.get(ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions): Promise<ResourceContent>",
            "Read a session artifact by artifact:// URI or ArtifactRef and return a ResourceContent envelope.",
            json!({
                "type": "object",
                "required": ["ref"],
                "additionalProperties": false,
                "properties": {
                    "ref": {},
                    "selector": { "type": "string", "description": "Optional 1-based inclusive line range." }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Read an artifact".to_string()),
                code: "const ref = artifacts.put('one\\ntwo');\nconst content = await artifacts.get(ref, { selector: '1-1' });".to_string(),
                notes: None,
            }],
            resource_errors("artifacts.get"),
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Reads use the resources.read:artifact grant.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.slice",
            "artifacts",
            "Read a selected artifact slice",
            "artifacts.slice(ref: ArtifactUri | ArtifactRef, selector: ResourceSelector): Promise<ResourceContent>",
            "Read a selected range from a session artifact.",
            json!({
                "type": "object",
                "required": ["ref", "selector"],
                "additionalProperties": false,
                "properties": {
                    "ref": {},
                    "selector": { "type": "string" }
                }
            }),
            Some(resource_content_schema()),
            vec![ToolExample {
                title: Some("Slice an artifact".to_string()),
                code: "const ref = artifacts.put('one\\ntwo');\nconst line = await artifacts.slice(ref, '2-2');".to_string(),
                notes: None,
            }],
            resource_errors("artifacts.slice"),
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Reads use the resources.read:artifact grant.".to_string(),
            }],
        ),
        core_doc(
            "artifacts.list",
            "artifacts",
            "List session artifacts",
            "artifacts.list(): ArtifactRef[]",
            "List artifact handles created in the current session.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
            Some(json!({ "type": "array", "items": artifact_ref_schema() })),
            vec![ToolExample {
                title: Some("List artifacts".to_string()),
                code: "const refs = artifacts.list();\ndisplay(refs, { kind: 'json' });".to_string(),
                notes: None,
            }],
            vec![tool_error("HostCallError", "The artifact store cannot list artifacts.", false)],
            vec![GrantDoc {
                kind: "artifact".to_string(),
                description: "Session-local artifact listing is always available inside the sandbox.".to_string(),
            }],
        ),
    ]
    .into_iter()
    .map(|docs| (docs.name.clone(), docs))
    .collect()
}

fn core_doc(
    name: &str,
    namespace: &str,
    summary: &str,
    signature: &str,
    description: &str,
    args_schema: Value,
    result_schema: Option<Value>,
    examples: Vec<ToolExample>,
    errors: Vec<ToolErrorDoc>,
    grants: Vec<GrantDoc>,
) -> ToolDocs {
    ToolDocs {
        name: name.to_string(),
        namespace: namespace.to_string(),
        summary: summary.to_string(),
        description: Some(description.to_string()),
        signature: signature.to_string(),
        args_schema,
        result_schema,
        examples,
        errors,
        grants,
        sensitive: false,
        approval: "none".to_string(),
        since: "M1".to_string(),
        stability: "experimental".to_string(),
    }
}

fn resource_errors(capability: &str) -> Vec<ToolErrorDoc> {
    vec![
        tool_error(
            "CapabilityDeniedError",
            &format!("{capability} is not granted for the requested resource scheme."),
            false,
        ),
        tool_error(
            "NotFoundError",
            "The resource or artifact does not exist.",
            false,
        ),
        tool_error(
            "InvalidArgsError",
            "The URI, selector, or arguments are malformed.",
            false,
        ),
    ]
}

fn tool_error(name: &str, when: &str, retryable: bool) -> ToolErrorDoc {
    ToolErrorDoc {
        name: name.to_string(),
        when: when.to_string(),
        retryable,
    }
}

fn resource_content_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "kind", "mime", "sizeBytes", "hasMore", "content", "preview"],
        "properties": {
            "uri": { "type": "string" },
            "kind": { "type": "string" },
            "mime": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": "integer" },
            "selector": { "type": ["string", "null"] },
            "hasMore": { "type": "boolean" },
            "content": { "type": "string" },
            "preview": { "type": "string" }
        }
    })
}

fn resource_entry_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "name", "kind"],
        "properties": {
            "uri": { "type": "string" },
            "name": { "type": "string" },
            "kind": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": ["integer", "null"] },
            "modifiedAt": { "type": ["string", "null"] }
        }
    })
}

fn artifact_ref_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "id", "kind", "mime", "sizeBytes", "preview"],
        "properties": {
            "uri": { "type": "string" },
            "id": { "type": "string" },
            "kind": { "type": "string" },
            "mime": { "type": "string" },
            "title": { "type": ["string", "null"] },
            "sizeBytes": { "type": "integer" },
            "preview": { "type": "string" }
        }
    })
}

fn core_doc_granted(name: &str, ctx: &InvocationCtx) -> bool {
    match name {
        "artifacts.put" | "artifacts.list" | "resources.list" => true,
        "artifacts.get" | "artifacts.slice" => ctx.grants.permits("resources.read:artifact"),
        "resources.read" | "resources.preview" => ctx
            .grants
            .names()
            .any(|grant| grant.starts_with("resources.read:")),
        _ => ctx.grants.permits(name),
    }
}

fn core_doc_matches(docs: &ToolDocs, query: &str, namespace: Option<&str>) -> bool {
    if let Some(namespace) = namespace
        && docs.namespace != namespace
    {
        return false;
    }
    let needle = query.to_lowercase();
    let haystack = format!("{} {} {}", docs.name, docs.namespace, docs.summary).to_lowercase();
    needle.is_empty() || haystack.contains(&needle)
}

#[op2]
#[serde]
async fn op_tm_host_call(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] args: serde_json::Value,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    sdk_result(
        host_state
            .host_registry
            .invoke(&name, args, &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
async fn op_tm_resource_read(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
    #[string] selector: String,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    let selector = (!selector.is_empty()).then_some(selector);
    sdk_result(
        host_state
            .resource_registry
            .read(&uri, selector.as_deref(), &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
async fn op_tm_resource_preview(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    sdk_result(
        host_state
            .resource_registry
            .preview(&uri, &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
async fn op_tm_resource_list(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    let uri = (!uri.is_empty()).then_some(uri);
    sdk_result(
        host_state
            .resource_registry
            .list(uri.as_deref(), &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
fn op_tm_tools_search(
    state: &mut OpState,
    #[string] query: String,
    #[serde] opts: serde_json::Value,
) -> Vec<ToolSummary> {
    let host_state = state.borrow::<RuntimeHostState>().clone();
    let namespace = opts
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit = (opts.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize).max(1);
    let mut summaries = host_state.host_registry.search(
        &query,
        namespace.as_deref(),
        limit,
        &host_state.invocation_ctx,
    );
    let core_docs = core_tool_docs();
    for docs in core_docs.values() {
        if summaries.len() >= limit {
            break;
        }
        if summaries.iter().any(|summary| summary.name == docs.name) {
            continue;
        }
        if !core_doc_matches(docs, &query, namespace.as_deref()) {
            continue;
        }
        summaries.push(ToolSummary {
            name: docs.name.clone(),
            namespace: docs.namespace.clone(),
            summary: docs.summary.clone(),
            sensitive: docs.sensitive,
            granted: core_doc_granted(&docs.name, &host_state.invocation_ctx),
        });
    }
    summaries
}

#[op2]
#[serde]
fn op_tm_tools_docs(state: &mut OpState, #[string] name: String) -> serde_json::Value {
    let host_state = state.borrow::<RuntimeHostState>().clone();
    let mut core_docs = core_tool_docs();
    let docs = host_state
        .host_registry
        .docs(&name, &host_state.invocation_ctx)
        .or_else(|err| match core_docs.remove(&name) {
            Some(docs) => Ok(docs),
            None => Err(err),
        });
    sdk_result(docs)
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

fn sdk_result<T: Serialize>(result: std::result::Result<T, HostError>) -> Value {
    match result {
        Ok(value) => sdk_ok(value),
        Err(err) => json!({
            "ok": false,
            "error": err.to_payload()
        }),
    }
}

fn sdk_ok<T: Serialize>(value: T) -> Value {
    match serde_json::to_value(value) {
        Ok(value) => json!({
            "ok": true,
            "value": value
        }),
        Err(err) => json!({
            "ok": false,
            "error": HostError::HostCall(err.to_string()).to_payload()
        }),
    }
}

fn js_error(err: impl ToString) -> JsErrorBox {
    JsErrorBox::generic(err.to_string())
}

extension!(
    tm_sandbox_ops,
    ops = [
        op_tm_host_call,
        op_tm_resource_read,
        op_tm_resource_preview,
        op_tm_resource_list,
        op_tm_artifact_put,
        op_tm_artifact_list,
        op_tm_tools_search,
        op_tm_tools_docs
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
    pub linked_folders: Option<LinkedFolders>,
    pub approval_policy: Arc<dyn ApprovalPolicy>,
    pub approval_timeout: Duration,
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
            linked_folders: None,
            approval_policy: Arc::new(DefaultDenyApprovalPolicy),
            approval_timeout: Duration::from_secs(60),
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
            ),
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
const __tm_sdk_error = (payload) => {
  const info = payload && typeof payload === "object" ? payload : {};
  const err = new Error(String(info.message ?? "host call failed"));
  err.name = String(info.name ?? "HostCallError");
  if (info.capability != null) err.capability = String(info.capability);
  if (info.path != null) err.path = String(info.path);
  if (info.uri != null) err.uri = String(info.uri);
  err.retryable = Boolean(info.retryable);
  err.details = info.details ?? null;
  return err;
};
const __tm_unwrap = (result) => {
  if (result && typeof result === "object" && result.ok === false) {
    throw __tm_sdk_error(result.error);
  }
  if (result && typeof result === "object" && result.ok === true) {
    return result.value;
  }
  return result;
};
const __tm_host_call = async (name, args) => __tm_unwrap(await __tm_ops.op_tm_host_call(name, args));
const __tm_resource_read = async (uri, selector) => __tm_unwrap(await __tm_ops.op_tm_resource_read(uri, selector));
const __tm_resource_preview = async (uri) => __tm_unwrap(await __tm_ops.op_tm_resource_preview(uri));
const __tm_resource_list = async (uri) => __tm_unwrap(await __tm_ops.op_tm_resource_list(uri));
globalThis.artifacts = {
  put: (data, opts = undefined) => __tm_sdk_shape(__tm_ops.op_tm_artifact_put(data, opts ?? null)),
  get: async (ref, opts = undefined) => __tm_sdk_shape(await __tm_resource_read(__tm_uri(ref), __tm_selector(opts))),
  slice: async (ref, selector) => artifacts.get(ref, { selector }),
  list: () => __tm_ops.op_tm_artifact_list().map(__tm_sdk_shape)
};
globalThis.resources = {
  read: async (uri, selector = undefined) => __tm_sdk_shape(await __tm_resource_read(String(uri), __tm_arg_selector(selector))),
  preview: async (uri) => __tm_sdk_shape(await __tm_resource_preview(String(uri))),
  list: async (uri = undefined) => (await __tm_resource_list(uri == null ? "" : String(uri))).map(__tm_sdk_shape)
};
globalThis.tools = {
  search: async (query, opts = undefined) => __tm_ops.op_tm_tools_search(String(query), opts ?? null),
  docs: async (name) => __tm_unwrap(__tm_ops.op_tm_tools_docs(String(name))),
  call: async (name, args = {}) => __tm_host_call(String(name), args ?? null)
};
globalThis.fs = {
  read: async (path, opts = undefined) => __tm_sdk_shape(await tools.call("fs.read", { path: String(path), ...(opts ?? {}) })),
  write: async (path, data, opts = undefined) => __tm_sdk_shape(await tools.call("fs.write", { path: String(path), data, ...(opts ?? {}) })),
  ls: async (path = undefined, opts = undefined) => await tools.call("fs.ls", { ...(path == null ? {} : { path: String(path) }), ...(opts ?? {}) }),
  find: async (patterns, opts = undefined) => await tools.call("fs.find", { patterns, ...(opts ?? {}) })
};
globalThis.code = {
  search: async (query) => await tools.call("code.search", query),
  edit: async (patch, opts = undefined) => await tools.call("code.edit", { ...patch, ...(opts ?? {}) })
};
globalThis.proc = {
  run: async (cmd, args = [], opts = undefined) => __tm_sdk_shape(await tools.call("proc.run", { cmd: String(cmd), args, ...(opts ?? {}) }))
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
    use std::fs;

    use tm_host::{FsMode, LinkedFolderConfig, LinkedFolders};

    fn p0_sandbox(root: &std::path::Path, artifact_root: &std::path::Path) -> DenoSandbox {
        DenoSandbox::new(DenoSandboxOptions {
            artifact_root: artifact_root.to_path_buf(),
            linked_folders: Some(
                LinkedFolders::from_configs(vec![LinkedFolderConfig {
                    name: "tempestmiku".to_string(),
                    path: root.to_path_buf(),
                    mode: FsMode::Rw,
                    commands: vec!["cargo".to_string()],
                    safe_args: vec![vec!["cargo".to_string(), "test".to_string()]],
                }])
                .unwrap(),
            ),
            ..DenoSandboxOptions::default()
        })
    }

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
        let sdk_types = include_str!("../../../docs/sdk/tm-runtime.d.ts");
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

        let docs = session
            .eval(
                "const artifactDocs = await tools.docs('artifacts.put');\n\
                 const resourceDocs = await tools.docs('resources.read');\n\
                 const found = await tools.search('artifact', { namespace: 'artifacts', limit: 10 });\n\
                 ({ artifactSignature: artifactDocs.signature, resourceSignature: resourceDocs.signature, artifactResultRequired: artifactDocs.resultSchema.required[0], resourceContentType: resourceDocs.resultSchema.properties.content.type, foundNames: found.map(item => item.name), putGranted: found.find(item => item.name === 'artifacts.put').granted })",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let result = docs.result.unwrap();
        assert_eq!(
            result["artifactSignature"],
            Value::String(
                "artifacts.put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef".into()
            )
        );
        assert_eq!(
            result["resourceSignature"],
            Value::String(
                "resources.read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>"
                    .into()
            )
        );
        assert!(
            sdk_types.contains(result["artifactSignature"].as_str().unwrap()),
            "docs/sdk/tm-runtime.d.ts is missing the artifacts.put signature"
        );
        assert!(
            sdk_types.contains(result["resourceSignature"].as_str().unwrap()),
            "docs/sdk/tm-runtime.d.ts is missing the resources.read signature"
        );
        assert_eq!(
            result["artifactResultRequired"],
            Value::String("uri".into())
        );
        assert_eq!(
            result["resourceContentType"],
            Value::String("string".into())
        );
        assert_eq!(result["putGranted"], Value::Bool(true));
        assert!(
            result["foundNames"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name.as_str() == Some("artifacts.get"))
        );
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
    async fn deno_host_errors_are_structured_js_errors() {
        let sandbox = DenoSandbox::default();
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "const err = await tools.call('missing.capability', {}).catch((err) => ({ name: err.name, message: err.message, capability: err.capability, retryable: err.retryable, details: err.details }));\n\
                 err",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let result = out.result.unwrap();
        assert_eq!(
            result["name"],
            Value::String("CapabilityDeniedError".into())
        );
        assert_eq!(
            result["capability"],
            Value::String("missing.capability".into())
        );
        assert_eq!(result["retryable"], Value::Bool(false));
        assert_eq!(
            result["details"]["capability"],
            Value::String("missing.capability".into())
        );
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("capability denied")
        );
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_http_get_is_default_deny_and_allowlisted() {
        let sdk_types = include_str!("../../../docs/sdk/tm-runtime.d.ts");
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

        let docs = session
            .eval(
                "const found = await tools.search('http', { namespace: 'http' });\n\
                 const docs = await tools.docs('http.get');\n\
                 const unknown = await tools.call('http.post', {}).catch(err => ({ name: err.name, capability: err.capability, retryable: err.retryable }));\n\
                 ({ found: found.map(item => ({ name: item.name, granted: item.granted, sensitive: item.sensitive })), signature: docs.signature, description: docs.description, grantKind: docs.grants[0].kind, deniedName: unknown.name, deniedCapability: unknown.capability, deniedRetryable: unknown.retryable })",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let result = docs.result.unwrap();
        assert_eq!(result["found"][0]["name"], Value::String("http.get".into()));
        assert_eq!(result["found"][0]["granted"], Value::Bool(true));
        assert_eq!(result["found"][0]["sensitive"], Value::Bool(true));
        assert_eq!(
            result["signature"],
            Value::String("http.get(url: string): Promise<string>".into())
        );
        assert!(
            sdk_types.contains(result["signature"].as_str().unwrap()),
            "docs/sdk/tm-runtime.d.ts is missing the http.get signature"
        );
        assert!(
            result["description"]
                .as_str()
                .unwrap()
                .contains("production egress policy remains deferred")
        );
        assert_eq!(result["grantKind"], Value::String("network".into()));
        assert_eq!(
            result["deniedName"],
            Value::String("CapabilityDeniedError".into())
        );
        assert_eq!(
            result["deniedCapability"],
            Value::String("http.post".into())
        );
        assert_eq!(result["deniedRetryable"], Value::Bool(false));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_p0_sdk_exposes_linked_repo_functions() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn edit() -> i32 { 1 }\n",
        )
        .unwrap();
        let sandbox = p0_sandbox(root.path(), artifacts.path());
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "const found = await tools.search('edit');\n\
                 const docs = await tools.docs('code.edit');\n\
                 const fsDocs = await tools.docs('fs.read');\n\
                 const read = await fs.read('tempestmiku:src/lib.rs');\n\
                 const listed = await fs.ls('tempestmiku:src');\n\
                 const hits = await code.search({ pattern: 'edit', paths: ['tempestmiku:src/lib.rs'], regex: false });\n\
                 const linked = await resources.read('linked://tempestmiku/src/lib.rs');\n\
                 ({ found: found.length, docName: docs.name, fsSignature: fsDocs.signature, fsRequired: fsDocs.argsSchema.required[0], fsResultContent: fsDocs.resultSchema.properties.content.type, fsExamples: fsDocs.examples.length, fsApproval: fsDocs.approval, readHasMore: read.hasMore, sizeBytes: listed[0].sizeBytes, hits: hits.length, linked: linked.content.includes('edit'), fsType: typeof fs, codeType: typeof code, procType: typeof proc, memoryType: typeof memory, skillsType: typeof skills, agentsType: typeof agents })",
                CellBudget::default(),
            )
            .await
            .unwrap();
        let result = out.result.unwrap();
        assert_eq!(result["docName"], Value::String("code.edit".into()));
        assert_eq!(
            result["fsSignature"],
            Value::String(
                "fs.read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>".into()
            )
        );
        assert_eq!(result["fsRequired"], Value::String("path".into()));
        assert_eq!(result["fsResultContent"], Value::String("string".into()));
        assert!(result["fsExamples"].as_u64().unwrap() > 0);
        assert_eq!(result["fsApproval"], Value::String("none".into()));
        assert_eq!(result["readHasMore"], Value::Bool(false));
        assert!(result["sizeBytes"].as_u64().unwrap() > 0);
        assert_eq!(result["hits"], Value::Number(1.into()));
        assert_eq!(result["linked"], Value::Bool(true));
        assert_eq!(result["fsType"], Value::String("object".into()));
        assert_eq!(result["codeType"], Value::String("object".into()));
        assert_eq!(result["procType"], Value::String("object".into()));
        assert_eq!(result["memoryType"], Value::String("undefined".into()));
        assert_eq!(result["skillsType"], Value::String("undefined".into()));
        assert_eq!(result["agentsType"], Value::String("undefined".into()));
    }

    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn deno_p0_linked_repo_patch_and_proc_run_through_sdk() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"p0-sdk-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn answer() -> i32 { 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn answer_is_two() {\n        assert_eq!(super::answer(), 2);\n    }\n}\n",
        )
        .unwrap();
        let sandbox = p0_sandbox(root.path(), artifacts.path());
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();
        let out = session
            .eval(
                "const hits = await code.search({ pattern: '1', paths: ['tempestmiku:src/lib.rs'], regex: false });\n\
                 const tag = hits[0].tag;\n\
                 await code.edit({ path: 'tempestmiku:src/lib.rs', tag, hunks: [{ op: 'replace', startLine: 1, endLine: 1, lines: ['pub fn answer() -> i32 { 2 }'] }] });\n\
                 const invalid = await proc.run('cargo test', [], { cwd: 'tempestmiku:' }).catch(err => String(err));\n\
                 const run = await proc.run('cargo', ['test'], { cwd: 'tempestmiku:' });\n\
                 ({ exitCode: run.exitCode, invalid })",
                CellBudget {
                    wall_ms: 240_000,
                    output_bytes: 50_000,
                },
            )
            .await
            .unwrap();
        let result = out.result.unwrap();
        assert_eq!(result["exitCode"], Value::Number(0.into()));
        assert!(
            result["invalid"]
                .as_str()
                .unwrap()
                .contains("InvalidArgsError")
        );
        let changed = fs::read_to_string(root.path().join("src/lib.rs")).unwrap();
        assert!(changed.contains("pub fn answer() -> i32 { 2 }"));
    }
}
