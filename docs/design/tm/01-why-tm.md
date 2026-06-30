# 1. Why `tm` — the bet

## 1.1 The observation

The §7 prelude is already a *castrated TypeScript*. No `Deno.*`, no `fetch`, no npm, no
`node:*`, no `globalThis.process`. `secrets`/`memory`/`skills`/`agents` are literally
`undefined`, and `http.get` is a default-deny allowlisted helper rather than ambient network.
The model uses maybe 30% of TS's surface; the other 70% is footguns and dead syntax that exists
only because TS is a general-purpose language we happened to pick.

Meanwhile, three things TempestMiku treats as **architectural invariants** (AGENTS.md) are
currently implemented as *runtime policy layered on top of the language*:

- **Capability gating / fail-closed** — reserved namespaces are unavailable and `op_host_call`
  checks grants at runtime.
- **Approval** — `ApprovalPolicy` (`on-write`/`on-external`/`always`) living in the
  orchestrator + host registry, three layers away from the code the model writes.
- **Provenance / replay** — §3 principle 6: "everything is replayable," recorded by the
  transcript, but the code has no idea which of its calls were pure vs effectful.

`tm`'s bet: **lift those three from runtime policy into language primitives.** Then the
capability set is a *type*, approval is a *control-flow primitive*, and provenance is a
*property of pure functions*. The code becomes self-describing about what it can do and what
it needs approved.

## 1.2 The three pillars

| Pillar | Maps to existing invariant | Language primitive |
|---|---|---|
| **Effectful** | §3.4 least-privilege, §3.9 one bridge | Algebraic effects + effect rows; host = handler |
| **Approval-aware** | §7 approval policy, AGENTS.md approval boundary | Resumable effects (suspend → ask → resume) |
| **Data-oriented, small** | §5.4 result shaping, §3.3 context-as-budget | Pipelines, tables, JSON, errors-as-values |

Pillar 1 is prior art, not a discovery. The "algebraic effects + handler = host registry"
shape is already validated in Haskell's effect-library ecosystem: [`effectful`](https://github.com/haskell-effectful/effectful)
with [`ki`](https://github.com/awkward-squad/ki) structured concurrency, and
[`atelier-core`](https://github.com/atelier-hub/tricorder/tree/b21172e/atelier-core) is a
production catalog of exactly these effects (`Process`, `Conc`, `Cache`, `FileSystem`, …)
with the same perform → interpreter dispatch `tm` adopts. `tm` does not claim "effects as
capabilities" as novel. What it adds over a library: the effect **row** as a fail-closed
eval-time boundary rather than a post-hoc `:>` constraint (§3.1); **resumable** approval
effects the library layer cannot express (§3.2); and the row as a **replay provenance**
marker (§3.1). The load-bearing novelty is pillars 2 and 3; pillar 1 lifts a validated
shape into the language so they have a type system to sit in.

## 1.3 The aha

The genuinely agent-first idea is not the syntax. It is:

> **Approval is a resumable effect.** The model writes `@code.edit! {patch}` and the language
> *suspends the whole isolate*, the host asks the user, and execution *resumes* with the
> answer — no callbacks, no try/catch, no polling. Approval policy is an effect-handler
> implementation detail, not an API the model learns.

TypeScript cannot express this. `await` is not suspension across an approval boundary; it is
intra-event-loop. You can fake it with a host op that blocks, but the TS type still lies — it
says `Promise<void>`, identical to `fs.read`. In `tm`:

```
fun ship(patch) : <Code Edit!> Unit = @code.edit! {patch}
```

The `!` is in the type and at the call site; the `@` shows the host boundary. The model, the
host, and the transcript all see it.

The `!` is not a naming hint. It is a **handler-interface contract**: a `!` effect's handler
*must* be resumable (it may suspend and resume); a non-`!` effect's handler *must* be
synchronous (it may never suspend). "May suspend" at the call site is the policy's choice;
"can suspend at all" is the type's choice. This is the same shape as Rust's `async` —
`async` fn may or may not actually await, but a non-`async` fn is statically guaranteed not
to.

## 1.4 What it is not

- **Not a replacement for TS.** §6.1 stands. `tm` is a `Sandbox` backend (§3.7 pluggable
  everything), a sibling of the deno_core and CPython backends.
- **Not for humans.** No package manager, no build system, no `Cargo.toml` of macros. It
  exists to be *written by a model* and *audited by the host*.
- **Not a general-purpose language.** No classes, no prototypes, no macros, no FFI. If you
  need a library, the host registers a capability (§3.9); the language never grows a package
  ecosystem.

## 1.5 The honesty

The cost is named and it is the same one §6.1 already flagged when it rejected Rhai/Rune:
**model fluency.** If the model writes `tm` worse than it writes the TS prelude, the whole
thing is a vanity project. §5 sets the gate: a fluency benchmark (same prompt, 50 runs each,
compare success rate + token count) must show `tm` not-worse before it ships anywhere. Until
then this folder is a design exercise — which, per the user's instruction, is the point: it
should be **fun**.
