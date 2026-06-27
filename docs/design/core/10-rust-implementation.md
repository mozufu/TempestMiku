# 10. Rust implementation

### 10.1 Workspace layout

```
tempest-miku/
├── crates/
│   ├── tm-core/        # message types, the agent loop, result-shaping policy, config
│   ├── tm-llm/         # OpenAI-compatible client (chat + streaming), LlmClient trait
│   ├── tm-sandbox/     # Sandbox/Session traits + backends (deno, quickjs, py-subprocess)
│   ├── tm-host/        # host capability registry, ops, capability policy, secret broker
│   ├── tm-artifacts/   # content-addressed artifact store
│   ├── tm-mcp/         # MCP client -> imports external tools into the catalog
│   └── tm-trace/       # tracing, transcript record/replay
└── apps/
    └── tm-cli/         # binary: wiring, config, REPL/chat entrypoint
```

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

- **Async runtime:** Tokio. `deno_core` event loop driven on a dedicated thread per isolate;
  `eval` is `async` and cancel-safe (drop ⇒ terminate isolate).
- **Errors:** `thiserror` per crate, `anyhow` at the binary edge. Sandbox errors are *data*
  returned to the model, not host failures.
- **Serialization:** `serde` everywhere; the op boundary is `serde_json::Value`.
- **Config:** layered (defaults → file → env → flags); capabilities are config, not code.
