# 10. Rust implementation

### 10.1 Workspace layout

```
tempest-miku/
├── crates/
│   ├── tm-core/        # message types, the agent loop, result-shaping policy, config
│   ├── tm-llm/         # OpenAI-compatible client (chat + streaming), LlmClient trait
│   ├── tm-sandbox/     # Sandbox/Session traits + backends (stub, deno_core; more planned)
│   ├── tm-host/        # host capability registry, linked folders, approvals, resource handlers (§9.2)
│   ├── tm-artifacts/   # content-addressed artifact store
│   ├── tm-persona/     # mode labels, default scopes, voice caps, persona asset status
│   └── tm-server/      # axum sessions, SSE replay, approvals, project views, coding backends
└── apps/
    └── tm-cli/         # binary: wiring, config, REPL/chat entrypoint
```

Client scaffolds live under `clients/` (`miku_flutter` for Web/PWA now and Android later, plus web
smoke coverage). Planned product/support crates remain `tm-memory`, `tm-agents`, `tm-drive`,
`tm-mcp`, and `tm-trace`; they should be extracted only when the settled server/resource surface has
two concrete users.

### 10.2 Key types & traits

```rust
// tm-core
pub enum Role { System, User, Assistant, Tool }

pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub tool_calls: Vec<ToolCall>,        // assistant
    pub tool_call_id: Option<String>,     // tool result
}

pub enum ContentBlock { Text(String), Image { mime: String, data: Bytes }, Artifact(ArtifactRef) }
pub struct ToolCall { pub id: String, pub name: String, pub arguments: serde_json::Value }

#[async_trait]
pub trait LlmClient: Send + Sync {
    // Streaming is the one source of truth (day 1): deltas carry text + tool-call-arg fragments.
    async fn chat_stream(&self, req: &ChatRequest)
        -> Result<BoxStream<'static, Result<StreamEvent>>>;
    // Convenience: drain the stream into a complete turn. Default-implemented.
    async fn chat(&self, req: &ChatRequest) -> Result<AssistantTurn> { /* accumulate stream */ }
}

// streaming types (day 1)
pub enum StreamEvent {
    Text(String),                                   // assistant text fragment
    ToolCall { index: usize, id: Option<String>, name: Option<String>, arguments: Option<String> },
    Finish { reason: Option<String> },
    Usage(Usage),
}

pub struct AssistantTurn {                          // assembled from a delta stream
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

// The loop pushes streaming events here as they arrive; the CLI sink prints tokens live.
pub trait EventSink: Send + Sync {
    fn on_text(&self, _delta: &str) {}              // assistant token(s), streamed
    fn on_tool_call(&self, _name: &str) {}          // a tool call began
    fn on_cell_start(&self, _code: &str) {}         // sandbox eval starting
    fn on_cell_result(&self, _shaped: &str) {}      // shaped result returning to the model
    fn on_final(&self, _text: &str) {}              // final answer
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    async fn open(&self, cfg: SessionConfig) -> Result<Box<dyn Session>>;
}

#[async_trait]
pub trait Session: Send {
    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput>;
    async fn reset(&mut self) -> Result<()>;
}

pub struct EvalOutput {
    pub stdout: String,
    pub result: Option<serde_json::Value>,
    pub displays: Vec<Display>,
    pub error: Option<EvalError>,
    pub artifacts: Vec<ArtifactRef>,
    pub host_calls: Vec<HostCallRecord>,
    pub usage: CellUsage,
}

// tm-host
#[async_trait]
pub trait HostFn: Send + Sync {
    fn descriptor(&self) -> &HostFnDescriptor;   // name, summary, params, docs — for progressive disclosure
    async fn invoke(&self, args: serde_json::Value, ctx: &InvocationCtx) -> Result<serde_json::Value>;
}

pub struct Capabilities {
    pub net: NetPolicy,            // allow/deny domains, byte+req caps
    pub fs: FsPolicy,              // root, ro/rw
    pub secrets: SecretScope,      // which named secrets are resolvable, and their egress scope
    pub limits: ResourceLimits,    // wall, heap, output, egress
    pub approvals: ApprovalPolicy, // which capabilities require human sign-off
}

// resource resolution (§9.2) — one registry, scheme-keyed handlers
#[async_trait]
pub trait ResourceHandler: Send + Sync {
    fn scheme(&self) -> &str;                                       // the URI scheme it owns
    async fn read(&self, uri: &str, sel: Option<Selector>, ctx: &InvocationCtx)
        -> Result<ResourceRead>;                                    // paged like artifacts.slice (§9.1)
    async fn list(&self, _uri: Option<&str>, _ctx: &InvocationCtx)  // optional enumeration
        -> Result<Vec<ResourceEntry>> { Ok(vec![]) }
}

pub struct ResourceRead { pub mime: String, pub preview: String, pub body: Bytes, pub size: usize }

// subsystems register handlers at startup (late-binding, principle #9); read() dispatches by scheme.
pub struct ResourceRegistry { /* scheme -> Arc<dyn ResourceHandler> */ }
impl ResourceRegistry {
    pub fn register(&mut self, h: Arc<dyn ResourceHandler>) { /* keyed by h.scheme() */ }
    async fn read(&self, uri: &str, sel: Option<Selector>, ctx: &InvocationCtx)  // capability-gated (§08)
        -> Result<ResourceRead> { /* split scheme, dispatch, fail closed on unknown/denied */ }
}
```

### 10.3 The loop (sketch)

```rust
pub async fn run(agent: &Agent, user: Message, sink: &dyn EventSink) -> Result<String> {
    let mut msgs = vec![agent.system_prompt(), user];
    let mut session = agent.sandbox.open(agent.session_cfg()).await?;

    for _ in 0..agent.cfg.max_turns {
        // Stream the turn; tokens reach the sink (UI) the instant they arrive.
        let mut stream = agent.llm.chat_stream(&agent.request(&msgs)).await?;
        let mut acc = Accumulator::new();
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if let StreamEvent::Text(t) = &ev { sink.on_text(t); }
            acc.push(ev);
        }
        let turn = acc.into_turn();
        msgs.push(turn.to_message());

        let Some(call) = turn.execute_call() else {
            sink.on_final(&turn.text);
            return Ok(turn.text);                          // no tool call ⇒ final answer
        };
        sink.on_cell_start(&call.code);
        let out    = session.eval(&call.code, agent.cell_budget()).await?;
        let shaped = agent.shape_result(out);
        sink.on_cell_result(&shaped);
        msgs.push(Message::tool_result(&call.id, shaped));
    }
    Err(Error::TurnBudgetExhausted)
}
```

### 10.4 Conventions

- **Async runtime:** Tokio. `deno_core` sessions are single-thread-affine and evaluated on their
  owning task/thread; a timeout helper terminates the V8 isolate on wall-clock budget expiry.
- **Errors:** `thiserror` per crate, `anyhow` at the binary edge. Sandbox errors are *data*
  returned to the model, not host failures.
- **Serialization:** `serde` everywhere; the op boundary is `serde_json::Value`.
- **Config:** layered (defaults → file → env → flags); capabilities are config, not code.


### 10.5 Current implementation boundary

The M0 stub path still exists, but only as a protocol-test and explicit `--stub-sandbox` fallback.
The default CLI path now wires the streaming `Agent` to `DenoSandbox`, backed by the same
`Sandbox` / `Session` traits. The runtime installs the first-pass JS/TS SDK prelude (`print`,
`display`, `tools`, `resources`, `artifacts`, `fs`, `code`, `proc`, plus default-deny allowlisted
`http.get`),
uses `tm-artifacts` for spill/readback, and calls host capabilities through `tm-host` grants,
resource handlers, and approval policy.

The server path now has two coding backends behind the same `CodingBackend` interface: the
replaceable P0a `omp_acp` bridge and the native Deno backend. The native Serious Engineer backend
maps approval-gated host calls such as unsafe `proc.run`, overwrites, and destructive edits into the
same HTTP approval broker used by the client API; timeout still denies by default. The general
non-coding chat runner keeps a conservative default-deny approval policy until a product surface owns
that manual flow.

That boundary remains deliberate:

- model-visible behavior still goes through one `execute(code)` tool;
- the loop never depends on stub- or Deno-specific behavior;
- product features target the trait boundary and host registry, not ambient runtime APIs;
- real-repo reach stays behind linked-folder grants, `code.edit`, and argv-vector `proc.run`;
- the stub remains useful for deterministic loop tests without creating a second agent loop.
