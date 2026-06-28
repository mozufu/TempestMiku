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
// injected prelude (illustrative; see §7 for the authoritative .d.ts)
const print = (...items: unknown[]) => Deno.core.ops.op_print(items);
const display = (value: unknown, opts?: DisplayOptions) =>
  Deno.core.ops.op_display(value, opts); // synchronous: buffered until cell finish

const tools = {
  search: (query: string, opts?: ToolSearchOptions) =>
    Deno.core.ops.op_tools_search(query, opts),
  docs: (name: string) => Deno.core.ops.op_tools_docs(name),
  call: (name: string, args?: JsonValue) =>
    Deno.core.ops.op_host_call(name, args),
};

const fs = {
  read: (path: SdkPath, opts?: FsReadOptions) =>
    tools.call("fs.read", { path, ...opts }),
  write: (path: SdkPath, data: string | Uint8Array | ArrayBuffer, opts?: FsWriteOptions) =>
    tools.call("fs.write", { path, data, ...opts }),
  ls: (path?: SdkPath, opts?: FsListOptions) =>
    tools.call("fs.ls", { path, ...opts }),
  find: (patterns: string | string[], opts?: FsFindOptions) =>
    tools.call("fs.find", { patterns, ...opts }),
};

const code = {
  search: (query: CodeSearchQuery) => tools.call("code.search", query),
  edit: (patch: PatchEdit, opts?: CodeEditOptions) =>
    tools.call("code.edit", { patch, ...opts }),
};

const proc = {
  run: (cmd: string, args: string[] = [], opts?: ProcRunOptions) =>
    tools.call("proc.run", { cmd, args, ...opts }),
};

const resources = {
  read: (uri: ResourceUri, selector?: ResourceSelector) =>
    tools.call("resources.read", { uri, selector }),
  preview: (uri: ResourceUri) => tools.call("resources.preview", { uri }),
  list: (uri?: ResourceUri) => tools.call("resources.list", { uri }),
};

const artifacts = {
  put: (data: ArtifactInput, opts?: ArtifactPutOptions) =>
    tools.call("artifacts.put", { data, ...opts }),
  get: (ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions) =>
    tools.call("artifacts.get", { ref, ...opts }),
  slice: (ref: ArtifactUri | ArtifactRef, selector: ResourceSelector) =>
    tools.call("artifacts.slice", { ref, selector }),
  list: () => tools.call("artifacts.list"),
};

// Reserved future namespaces are explicitly present but unavailable in the first pass.
globalThis.http = undefined;
globalThis.secrets = undefined;
globalThis.memory = undefined;
globalThis.skills = undefined;
globalThis.agents = undefined;
```

**Two tiers of ops.** A small fixed set of **core primitives** (`print`, synchronous
`display`, `artifacts`, `resources`) is bound directly; every other capability goes through one
**dispatch bridge** (`op_host_call` → registry, surfaced as `tools.call`). New capabilities take the
bridge — register a handler, emit a typed stub — so the op layer never grows and never needs a
rebuild (§3 principle 9). The first real JS/TS pass exposes `fs.*`, `code.search`, JSON-hunk
`code.edit`, and argv-vector `proc.run`; `http`, `secrets`, `memory`, `skills`, and `agents` are set
to `undefined` until their backing crates and policies exist. If a future namespace exists but a
method is not ready, it throws `NotImplementedError`.

For an **out-of-process** backend (Python), the same SDK is implemented over **JSON-RPC** on a
pipe/socket: the in-sandbox `host.*` makes a blocking RPC, the Rust host services it
asynchronously and concurrently, and replies. Code stays simple (no async needed); concurrency
lives host-side or behind an explicit `host.parallel([...])`.

### 6.5 Async & parallelism

`deno_core` runs a Tokio-backed event loop, so the model's code can issue concurrent host calls
naturally:

```ts
const docs = await Promise.all(paths.map(p => fs.read(p))); // fan-out, one execute() round-trip
const hits = docs.flatMap(d => d.content?.split("\n") ?? []).filter(line => line.includes("TODO"));
display(hits.slice(0, 20), { kind: "json", title: "first TODO lines" });
```

### 6.6 Future backend: Python in isolation

When fluency demands Python: spawn CPython in a locked-down subprocess (Linux: namespaces +
seccomp + cgroups; or gVisor/Firecracker microVM), bridge via JSON-RPC. macOS lacks
production-grade local sandboxing — for dev, rely on subprocess + rlimits + capability-gated host
ops; for prod, run the Linux isolation path. Same `Sandbox` trait.
