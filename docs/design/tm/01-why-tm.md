# 1. Why `tm` — the bet

## 1.1 The observation

The §7 prelude is already a *castrated TypeScript*. No `Deno.*`, no `fetch`, no npm, no
`node:*`, no `globalThis.process`. `http`/`secrets`/`memory`/`skills`/`agents` are literally
`undefined`. The model uses maybe 30% of TS's surface; the other 70% is footguns and dead
syntax that exists only because TS is a general-purpose language we happened to pick.

Meanwhile, three things TempestMiku treats as **architectural invariants** (AGENTS.md) are
currently implemented as *runtime policy layered on top of the language*:

- **Capability gating / fail-closed** — `globalThis.http = undefined` and `op_host_call`
  checking grants at runtime.
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

## 1.3 The aha

The genuinely agent-first idea is not the syntax. It is:

> **Approval is a resumable effect.** The model writes `code.edit {patch}` and the language
> *suspends the whole isolate*, the host asks the user, and execution *resumes* with the
> answer — no callbacks, no try/catch, no polling. Approval policy is an effect-handler
> implementation detail, not an API the model learns.

TypeScript cannot express this. `await` is not suspension across an approval boundary; it is
intra-event-loop. You can fake it with a host op that blocks, but the *type* of `code.edit`
still lies — it says `Promise<void>`, identical to `fs.read`. In `tm`:

```
fun ship(patch) : {Code Edit!} Unit := code.edit {patch}
```

The `!` is in the type. The model, the host, and the transcript all see it.

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
