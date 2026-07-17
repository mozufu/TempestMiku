# 1. Why `tm` — the bet

## 1.1 The observation

The §7 prelude is already a *castrated TypeScript*. No `Deno.*`, no `fetch`, no npm, no
`node:*`, no `globalThis.process`. `secrets`/`memory`/`skills`/`agents` are literally
`undefined`, and `http.get` is a default-deny allowlisted helper rather than ambient network.
The model uses maybe 30% of TS's surface; the other 70% is footguns and dead syntax that exists
only because TS is a general-purpose language we happened to pick.

Meanwhile, four things TempestMiku treats as **architectural invariants or product contracts**
(AGENTS.md, §5, §7, §27) currently live *outside the language surface*:

- **Capability gating / fail-closed** — reserved namespaces are unavailable and `op_host_call`
  checks grants at runtime.
- **Approval** — `ApprovalPolicy` (`on-write`/`on-external`/`always`) living in the
  orchestrator + host registry, three layers away from the code the model writes.
- **Provenance / replay** — §3 principle 6: "everything is replayable," recorded by the
  transcript, but the code has no idea which of its calls were pure vs effectful.
- **Streaming UI** — the client sees runtime events, but general-purpose TS does not expose the
  structural relationship between a cell, a `par` scope, its child calls, approval suspension,
  and the value eventually presented.

`tm`'s bet: **give those contracts one language-shaped execution model.** Then the capability
set is a *type*, approval is a resumable *runtime state*, provenance is a property of pure
functions, and execution structure is a stream the UI can render without parsing source or
guessing from generic tool calls. The code becomes self-describing about what it can do; the
registry and actual runtime policy remain the authority on what needs approval.

## 1.2 The four pillars

| Pillar | Maps to existing invariant | Language primitive |
|---|---|---|
| **Effectful** | §3.4 least-privilege, §3.9 one bridge | Algebraic effects + inferred effect rows; host = handler |
| **Approval-aware** | §7 approval policy, AGENTS.md approval boundary | Resumable effects (suspend → ask → resume) |
| **Data-oriented, small** | §5.4 result shaping, §3.3 context-as-budget | Pipelines, tables, JSON, error effects |
| **Observable** | §5 streaming, §12 replay, §27 clients | Structured cell/effect lifecycle + declarative presentation |

Pillar 1 is prior art, not a discovery. The "algebraic effects + handler = host registry"
shape is already validated in Haskell's effect-library ecosystem: [`effectful`](https://github.com/haskell-effectful/effectful)
with [`ki`](https://github.com/awkward-squad/ki) structured concurrency, and
[`atelier-core`](https://github.com/atelier-hub/tricorder/tree/b21172e/atelier-core) is a
production catalog of exactly these effects (`Process`, `Conc`, `Cache`, `FileSystem`, …)
with the same perform → interpreter dispatch `tm` adopts. `tm` does not claim "effects as
capabilities" as novel. What it adds over a library: the inferred effect **row** as a fail-closed
eval-time boundary rather than a post-hoc `:>` constraint (§3.1); **resumable** approval
effects the library layer cannot express (§3.2); and the row as a **replay provenance**
marker (§3.1). The load-bearing novelty is approval suspension plus the shared replay/UI trace;
pillar 1 lifts a validated shape into the language so those features have a type system and
execution structure to sit in.

## 1.3 The aha

The genuinely agent-first idea is not the syntax. It is:

> **Approval is a resumable effect.** The model writes `@fs.remove target` and the language
> *suspends the whole isolate*, the host asks the user, and execution *resumes* with the
> answer — no callbacks, no try/catch, no polling. Approval policy is an effect-handler
> implementation detail, not an API the model learns.

TypeScript cannot express this. `await` is not suspension across an approval boundary; it is
intra-event-loop. You can fake it with a host op that blocks, but the TS type still lies — it
says `Promise<void>`, identical to `fs.read`. In `tm`:

```
fun remove target = @fs.remove target
```

The checker infers the concrete authority row (fs.remove here) and possible error set, then emits
them through the typed-cell artifact and trace. @ shows the host boundary. The registry declares
that fs.remove is resumable and carries always-approve metadata; the effective runtime policy
decides whether this particular perform suspends. The transcript records what actually happened.

There is deliberately no call-site `!`. Approval policy is not part of capability identity,
and punctuation would be misleading when a pre-approved write does not prompt or a normally
read-only external call does. If a future workflow needs to demand fresh human confirmation
regardless of configured policy, that should be an explicit control effect such as an
`approve ... then ...` form, not a suffix on the capability name.

## 1.4 What it is not

- **The sole language runtime.** `tm` is the shipped `Sandbox` implementation (§3.7 pluggable
  boundary); the former language backends were retired after cutover.
- **Not for humans.** No package manager, no build system, no `Cargo.toml` of macros. It
  exists to be *written by a model* and *audited by the host*.
- **Not a general-purpose language.** No classes, no prototypes, no macros, no FFI. If you
  need a library, the host registers a capability (§3.9); the language never grows a package
  ecosystem.
- **Not a declarative or reactive planner.** A cell is evaluated now; a binding is a committed
  value, not a subscription that silently re-runs host effects when its inputs change.

## 1.5 The honesty

The cost is named and it is the same one §6.1 already flagged when it rejected Rhai/Rune:
**model fluency.** If the model writes `tm` worse than it writes the TS prelude, the whole
thing is a vanity project. §5 sets the gate: a fluency benchmark (same prompt, 50 runs each,
compare success rate + token count) must show `tm` not-worse before it ships anywhere. The
`tm-fluency-prompt-v2` run passed that gate on 2026-07-16: both backends achieved 1,000/1,000
first-try successes with zero retries, while tm generated shorter code. tm-lang therefore became
the sole runtime; the former fallback was removed after the cutover evidence passed.
