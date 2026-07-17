# 06 — tm REPL sandbox

## 6.1 Decision

`tm-conformance-v2`, implemented by `crates/tm-lang`, is the sole language/runtime behind
`execute(code)`. CLI and server do not expose a backend selector. The former Deno/V8 and shipped
stub implementations were deleted after tm-lang passed the frozen conformance, approval, replay,
client, and comparative-fluency gates.

The public abstraction remains `tm_core::Sandbox` / `tm_core::Session`; this keeps the agent loop
independent from interpreter details without promising another language backend.

## 6.2 Persistent cells

A session owns one persistent tm interpreter environment. Successful top-level bindings and named
functions commit atomically and remain available to later `execute` calls. Failed, denied, timed
out, cancelled, or otherwise incomplete cells roll back their pending bindings. `reset` restores
the frozen prelude and clears ephemeral state.

The checker runs before evaluation. Unknown names, invalid patterns, ungranted capabilities, and
unknown resource schemes fail closed. The runtime enforces wall, step, result, output, and
presentation limits through `CellBudget` plus tm runtime limits.

## 6.3 Effects and approvals

Host access is explicit tm effect syntax:

```tm
let hits = @code.search {pattern: "TODO", paths: ["repo:src"], regex: false};
hits |> take 10 |> display {kind: "table"}
```

Every effect is resolved through the existing `tm_host::HostRegistry`, exact per-turn grants,
`ApprovalPolicy`, resource registry, and artifact store. Registration never grants authority.
Approval-backed effects suspend the tm continuation and resume that same node after an approved
decision; denial, timeout, cancellation, stale resume, and duplicate completion fail closed.

The runtime emits bounded `effect_start`, `effect_suspended`, `effect_resumed`, `effect_result`,
scope, display, binding, cell-result, and runtime-reset events through the existing event sink.

## 6.4 Runtime ownership

tm sessions are `!Send`. Server chat and native coding backends shard sessions onto owning
single-thread Tokio runtimes, preserve session affinity, and destroy cached sessions on their owner
thread. Runtime loss creates a fresh interpreter and emits the existing reset/replay contract;
ephemeral bindings are not reconstructed from durable events.

## 6.5 Ambient authority

tm has no filesystem, process, network, environment, package-manager, or dynamic-code ambient
authority. Real-repo operations use linked-folder `fs.*` / `code.*`; processes use argv-vector
`proc.run`; network uses allowlisted host capabilities; large values spill to `artifact://`; and
resources are read only through registered schemes. One model-visible tool remains:
`execute({code})`.
