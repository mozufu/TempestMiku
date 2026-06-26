# 6. The REPL / sandbox

### 6.1 Language choice

Decision matrix for the language the *model* writes — the axes that actually decide it:

| Option | Model fluency | Rust embeddability | Sandbox story | npm/stdlib | Verdict |
|---|---|---|---|---|---|
| **JS/TS via `deno_core`** (V8) | High | Good (single crate) | Excellent — ops opt-in, zero ambient I/O, in-process | ES + web APIs you expose | **Chosen** |
| JS via `rquickjs` (QuickJS) | High | Excellent (small, fast startup) | Excellent — no I/O unless bound | ES2020-ish, no npm | Alt (tests / minimal builds) |
| Python via subprocess (CPython) | Highest | Out-of-process + RPC | Strong on Linux (ns+seccomp); needs a container on macOS | Full PyPI | **Planned 2nd backend** |
| Pure-Rust DSL (`rhai`/`rune`) | Low | Excellent | Excellent | Tiny | Rejected — poor fluency |

**Decision: TypeScript on `deno_core`** — the only substrate that is portably sandboxed,
zero-external-dependency, *and* cheap to extend, all at once.

- **Real async event loop** — host calls are async ops; the model can `await Promise.all([...])`
  for natural fan-out.
- **Capability model is free** — nothing is callable unless we register it (no `fetch`/fs/`Deno.*`
  by default). That *is* the least-privilege boundary, in-process and portable — no container on macOS.
- **Cheap to extend** — capabilities route through one dispatch bridge into a runtime registry
  (§3 principle 9): add a handler + a generated stub, no new op, no rebuild.
- **Ecosystem worry is mostly moot** — capabilities come from the Rust host, not a package manager,
  and there is no raw shell (§3 principle 8), so the `bash("python …")` escape never arises.
- **Good model priors** — high TS fluency; matches Anthropic's own TS examples.

`rquickjs` stays as a lighter, faster-start alternative for tests and minimal builds. If the model
ever genuinely needs in-language Python libraries (`pandas`, `pdfplumber`, …), the
**CPython-subprocess backend** (§6.6) drops in behind the same `Sandbox` trait — same loop, SDK,
and registry, different executor. The choice is reversible by construction.

### 6.2 Session & state

A `Session` is one long-lived V8 isolate + context. `eval(code)` runs a cell in that context;
top-level `let/const/var` and assignments persist. `reset()` tears down and recreates the isolate
for a clean slate. Sessions are **pooled** to amortize V8 startup; a fresh isolate is handed out
per agent run and returned/reset afterward.

### 6.3 Resource limits

Per cell and per session:

- **Wall-clock timeout** → isolate terminated, `Timeout` error returned to the model.
- **Heap cap** (V8 `--max-old-space-size` / isolate heap limit) → `OutOfMemory`.
- **Output cap** (stdout + return bytes) → truncation + artifact spill.
- **Egress cap** (bytes/requests via host net ops) → `EgressLimit`.
- **Host-call rate/quota** per capability.

A killed cell never kills the host; it returns a structured error the model can react to.

### 6.4 The host-call bridge

The SDK (a TS prelude injected at session creation) is thin sugar over **ops** — Rust async
functions registered with the isolate. Sketch (deno_core `op2`):

```rust
#[op2(async)]
#[serde]
async fn op_tools_call(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] args: serde_json::Value,
) -> Result<serde_json::Value, AnyError> {
    let registry = state.borrow().borrow::<Arc<HostRegistry>>().clone();
    let ctx = state.borrow().borrow::<InvocationCtx>().clone();
    registry.invoke(&name, args, &ctx).await   // capability checks happen inside invoke()
}
```

```ts
// injected prelude (illustrative)
const tools = {
  search: (q: string)            => Deno.core.ops.op_tools_search(q),
  docs:   (name: string)         => Deno.core.ops.op_tools_docs(name),
  call:   (name: string, a: any) => Deno.core.ops.op_tools_call(name, a),
};
const display = (v: unknown, o?: DisplayOpts) => Deno.core.ops.op_display(v, o);
const http = {
  get:  (url: string, init?: ReqInit) => Deno.core.ops.op_http(  "GET", url, init),
  post: (url: string, body: unknown, init?: ReqInit) => Deno.core.ops.op_http("POST", url, init, body),
};
const artifacts = {
  put:   (data: Uint8Array | string, o?: PutOpts) => Deno.core.ops.op_artifact_put(data, o),
  slice: (id: string, s: number, e: number)       => Deno.core.ops.op_artifact_slice(id, s, e),
};
const secrets = { use: (name: string) => Deno.core.ops.op_secret_handle(name) };
```

**Two tiers of ops.** A small fixed set of **core primitives** (`display`, `print`, `artifacts`)
is bound directly; every other capability goes through one **dispatch bridge**
(`op_host_call` → registry, surfaced as `tools.call`). New capabilities take the bridge — register
a handler, emit a stub — so the op layer never grows and never needs a rebuild (§3 principle 9).

For an **out-of-process** backend (Python), the same SDK is implemented over **JSON-RPC** on a
pipe/socket: the in-sandbox `host.*` makes a blocking RPC, the Rust host services it
asynchronously and concurrently, and replies. Code stays simple (no async needed); concurrency
lives host-side or behind an explicit `host.parallel([...])`.

### 6.5 Async & parallelism

`deno_core` runs a Tokio-backed event loop, so the model's code can issue concurrent host calls
naturally:

```ts
const pages = await Promise.all(urls.map(u => http.get(u)));   // fan-out, one execute() round-trip
const hits  = pages.flatMap(p => p.json().items).filter(x => x.score > 0.8);
display(hits.slice(0, 20));   // only the distilled 20 reach context
```

### 6.6 Future backend: Python in isolation

When fluency demands Python: spawn CPython in a locked-down subprocess (Linux: namespaces +
seccomp + cgroups; or gVisor/Firecracker microVM), bridge via JSON-RPC. macOS lacks
production-grade local sandboxing — for dev, rely on subprocess + rlimits + capability-gated host
ops; for prod, run the Linux isolation path. Same `Sandbox` trait.
