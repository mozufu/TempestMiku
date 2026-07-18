# TODO

Last aligned: **2026-07-17**.

Completed milestone: **tm-lang-only runtime hard cut**. Next active
milestone: **P7.2b approval-backed persona addenda**.

`ROADMAP.md` is the milestone-order and completion-history source. P8 is closed; its implementation,
quality metrics, lumo provenance, restart/fallback evidence, and exact gate matrix live in
[`docs/evidence/2026-07-15-p8-5-fuller-memory.md`](docs/evidence/2026-07-15-p8-5-fuller-memory.md).

P6.6 on-device ASR remains explicitly deferred. Preserve its interface spike and evidence, but do
not add microphone permission, production weights/runtime, server fallback, or automatic send unless
the owner explicitly resumes it.

## P7.2b Approval-backed Persona Addenda

- [x] Add a separate immutable managed persona-addendum catalog for typed tone, address, and
      interaction-preference guidance.
- [x] Keep durable manual approval as the only activation path, compose the active version on the
      next prompt across all modes, and preserve the hand-authored `SOUL.md` as authority.
- [x] Add a separate durable manual rollback to a prior proposal-backed version or the base persona.
- [ ] Add Auto-mode repeated-preference/persona-mismatch candidate detection over bounded P8
      evidence, with deterministic deduplication and cooldown.
- [ ] Close deny/timeout/stale/retry, restart, UI/e2e, and evidence gates; then mark P7.2b complete.

## tm-lang-only Runtime Outcome

tm-lang `tm-conformance-v2` is the only language accepted by `execute(code)` and the only in-process `Sandbox`
implementation shipped by CLI/server. The retired Deno/V8 and stub backends, backend selectors,
JavaScript SDK declaration, and executable TypeScript comparison runner are gone. Historical
fluency evidence and its frozen corpus remain under `docs/evidence/`; existing durable events are
not rewritten, while new native coding events use the `native-tm` backend label.

## tm-lang Runtime — Execution Plan

- [x] **T0 — freeze the milestone contract:** align `ROADMAP.md`, `AGENTS.md`, and §tm status with
      this owner-priority override; freeze a versioned §tm/07 conformance corpus and explicitly mark
      syntax outside the first end-to-end path as unsupported rather than silently accepting it.
- [x] **T1 — land the language crate:** create `crates/tm-lang` with a hand-written lexer, spanned
      AST, Pratt parser, and deterministic diagnostics. Cover the full frozen grammar with
      accept/reject fixtures.
- [x] **T2 — add the fail-closed checker:** infer value/function types plus capability, error, and
      presentation rows from a fake catalog; reject unknown or ungranted capabilities/schemes,
      invalid records/patterns, non-exhaustive closed matches, and escaping local sum types.
- [x] **T3 — close pure persistent-cell semantics:** implement JSON-shaped values, functions,
      patterns, pipelines, tables/prelude, local sum types, `do`, atomic top-level binding commit,
      reset, wall/step/output limits, and rollback on every failure path.
- [x] **T4 — make effects an explicit machine:** add a fake catalog/handler seam and a backend-local
      `start -> suspended -> resume|cancel -> complete` continuation contract with stable cell/node
      ids and ordered lifecycle events. Add `display`, bounded `print`, `par` scopes, sibling
      cancellation, stale-resume denial, and effect idempotency tests before deciding whether
      `tm-core::Session` itself must widen.
- [x] **T5 — adapt tm-lang to the real runtime:** implement a `Sandbox` backend that derives effect
      signatures and metadata from the existing `HostRegistry`, preflights exact grants, invokes the
      existing handlers/`ApprovalPolicy`, uses current artifacts/resources, and emits bounded events
      through the existing session-event path.
- [x] **T6 — prove the product seam:** add focused server/Postgres/e2e coverage for one persistent
      cell with `par`, an approval-gated `@fs.remove`, and table `display`; prove approve, deny,
      timeout, cancellation, reset/runtime loss, reconnect/replay, exact-once effect execution,
      atomic binding commit, redaction, and no authority widening.
- [x] **T7 — earn default status:** the historical §tm/05 20-prompt × 50-run TS-vs-tm corpus met its
      success/token/retry gates. After the tm-lang-only cut, the runner is retired and only its
      immutable evidence/corpus remains.

## tm-conformance-v2 Batch Hardening

- [x] Replace the model-confusing standalone `---` form separator with semicolon-separated cells;
      allow one trailing semicolon without discarding the final value, reject empty forms, and
      diagnose legacy separators as `TM1006` instead of treating them as comments.
- [x] Preserve the frozen v1 corpus as historical evidence while moving the active parser, checker,
      prompts, runtime-owned fluency skill, examples, clients, and accept/reject corpus to v2.
- [x] Replace speculative `TM3006` retries with scope-aware free-read/persistent-write analysis and
      a forward dependency DAG. Independent cells overlap; dependent cells execute once after
      producers commit; final state merges in response order.
- [x] Fail closed when a producer fails: dependent effects never execute, stale bindings are never
      substituted, and the durable trace records a stable `BatchDependencyError` cell failure.

## Regression Invariants

- Preserve Miku identity, mode routing, streaming-first turns, durable SSE replay, exact grants,
  manual approvals, and server-owned owner/scope authority.
- Preserve one model-visible `execute(code)` tool, one agent loop, and one host capability registry;
  tm-lang is a `Sandbox` backend, not a parallel runtime architecture.
- Preserve P8's bounded hybrid recall, inspectable evidence/corrections, exact retry context, and
  truthful lexical fallback. Retrieval scores and model inference are never owner truth.
- A suspended effect must resume the same continuation and remain origin-, approval-, and
  idempotency-bound; unknown effects/resources and stale continuations fail closed.

## Explicit Deferments

- **P6.6 on-device ASR:** deferred until the owner explicitly reopens the independent evidence note's
  quality, memory-headroom, production-flow, and signed-device resume gates.
- **P9/P10:** production egress and opaque-secret hardening precede MCP or live external research.
- Demand-triggered only: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and any future
  second sandbox backend.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.

## Next Execution Order

1. P7.2b approval-backed persona addenda.
2. P9 production egress and opaque secret broker.
3. P10 MCP catalog and live research through the P9 boundary.

P6.6 has no scheduled queue position while deferred.
