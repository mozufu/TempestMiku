# `tm` — an agent-first language for `execute(code)`

> Status: **implemented and sole runtime as of 2026-07-16**. `tm` is the `Sandbox` language that turns the runtime policies
> TempestMiku already needs — capability gating, approval, provenance, fail-closed — into
> language primitives, so the code the model writes *is itself* the auditable artifact.

The active source contract is frozen as `tm-conformance-v2`; syntax not present in §7 or its
versioned accept corpus is unsupported and must be rejected rather than guessed. The frozen v1
corpus remains historical evidence only. The pitch in one line:
**if every host interaction is an algebraic effect, every approval is
a resumable runtime state, and every cell emits a structured trace, then the host's capability
registry *is* the language's type system and the same execution record can drive both replay
and UI.**

T0-T7 are implemented and verified through the public HTTP/SSE, approval, runtime-loss, real
Postgres reconnect, affected-client, and frozen 20-prompt x 50-run comparative fluency paths.
The historical two-language system prompts were calibrated together and frozen as
`tm-fluency-prompt-v2`; that comparator runner is now retired.

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
7. [Language reference — the syntax contract](07-language-reference.md)

Read §6 (REPL/sandbox) and §7 (host SDK) of the core docs first; `tm` is a re-expression of
those invariants, not a replacement for them. The new §7 language reference is the canonical
source grammar; the syntax tour remains the model-facing introduction.
