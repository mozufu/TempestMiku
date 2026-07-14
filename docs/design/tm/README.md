# `tm` — an agent-first language for `execute(code)`

> Status: **experimental / fun**. This is a design exploration, not a committed milestone.
> It does **not** change §6.1's decision (TypeScript on `deno_core` is the shipping sandbox
> language). `tm` is a possible *third* `Sandbox` backend that turns the runtime policies
> TempestMiku already needs — capability gating, approval, provenance, fail-closed — into
> language primitives, so the code the model writes *is itself* the auditable artifact.

The pitch in one line: **if every host interaction is an algebraic effect, every approval is
a resumable runtime state, and every cell emits a structured trace, then the host's capability
registry *is* the language's type system and the same execution record can drive both replay
and UI.**

`tm` is a **persistent effectful REPL with declarative presentation and an observable execution
model**. It is not a generally declarative or reactive language: cells evaluate immediately,
successful bindings persist, and host effects happen in explicit dependency order. Pipelines
make data transformation read declaratively; effect lifecycle events make the work visible.

Sections:

1. [Why `tm` — the bet](01-why-tm.md)
2. [Syntax tour — the fun part](02-syntax-tour.md)
3. [Effects & approval — the load-bearing idea](03-effects-approval.md)
4. [Data, pipelines & errors](04-data-pipelines.md)
5. [Backend, fluency risk & where it lives](05-backend-fluency.md)
6. [Persistent REPL, execution trace & UI](06-repl-ui.md)

Read §6 (REPL/sandbox) and §7 (host SDK) of the core docs first; `tm` is a re-expression of
those invariants, not a replacement for them.
