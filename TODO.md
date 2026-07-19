# TODO

Last aligned: **2026-07-19**.

Completed in the 2026-07-18 roadmap pass: **P0a real OMP ACP dogfood**, **P2 actual-output live
voice parity**, **M2 `tools.call`**, **P7.2b approval-backed persona addenda**, **P9 production
egress/opaque secrets**, and **P10 selected MCP/live research**. P6.6 and full P6 are closed: exact
model activation, airplane-mode cold-start inspection, latest signed-app install/selector,
same-device synthetic A/B, lifecycle/failure/current/new canaries, and the consented production-local
10-item corpus pass. M4's bubblewrap and `linux_hardened_v1` seccomp/cgroup
profiles are implemented, canaried, and accepted on the persistent homolab production service.
The selected boundary is a hostile workload on a trusted owner-controlled host kernel; hostile
host-kernel containment and microVM isolation remain explicitly unclaimed.

`ROADMAP.md` is the milestone-order and completion-history source. P8 is closed; its implementation,
quality metrics, lumo provenance, restart/fallback evidence, and exact gate matrix live in
[`docs/evidence/2026-07-15-p8-5-fuller-memory.md`](docs/evidence/2026-07-15-p8-5-fuller-memory.md).

P6.6 was explicitly resumed and closed by the whole-roadmap request. Preserve foreground capture
and editable review. Local stays the default; the optional home-hosted engine is an explicit owner
choice through a fixed server destination and never an automatic fallback. No hotword/background capture or
automatic send is allowed. Preserve its completed status only with the signed-device matrix and
retained consented real-speaker evidence.

## P7.2b Approval-backed Persona Addenda

- [x] Add a separate immutable managed persona-addendum catalog for typed tone, address, and
      interaction-preference guidance.
- [x] Keep durable manual approval as the only activation path, compose the active version on the
      next prompt across all modes, and preserve the hand-authored `SOUL.md` as authority.
- [x] Add a separate durable manual rollback to a prior proposal-backed version or the base persona.
- [x] Add Auto-mode repeated-preference/persona-mismatch candidate detection over bounded P8
      evidence, with deterministic deduplication and cooldown.
- [x] Close deny/timeout/stale/retry, restart, UI/e2e, and evidence gates; then mark P7.2b complete.

## P9 Production Egress and Opaque Secrets

- [x] Add exact HTTPS destination, DNS/IP/rebinding/redirect, header, request/response/count/time,
      revocation, and fail-closed default policy.
- [x] Keep secret values host-only behind session/actor/destination-scoped opaque handles with
      bounded exact-literal response redaction and an explicit non-information-flow claim boundary.
- [x] Persist mutation intents and active-session budgets transactionally across restart and
      concurrent server instances; never resend an uncertain mutation.
- [x] Prove approval revalidation/digests, audit ordering, denial/replay, real PostgreSQL durability,
      and the opt-in live HTTPS transport.

## P10 Selected MCP and Live Research

- [x] Import exact selected tools/prompts/resources into lazy SDK and `mcp://` surfaces while keeping
      `execute(code)` as the only model-visible tool.
- [x] Implement exact MCP `2025-11-25` lifecycle/catalog pagination, strict bounded schema/result
      handling, Streamable HTTP JSON/SSE/session resume, and atomic catalog activation.
- [x] Preserve P9 authority, untrusted-data/provenance envelopes, manual local mutation policy, and
      durable mutation replay across equivalent catalog reloads and restart.
- [x] Pass deterministic, real-Postgres, public-API, and official Cloudflare documentation MCP
      read-only live canaries.

## Acceptance Gates

- [x] **P6.6 / full P6:** exact model activation, airplane-mode cold start, final-build fingerprint,
      the latest signed `1.0.3+4` in-place install/hash match, explicit self-hosted selector, one
      consented exact-reference review canary, and the same-installation streaming/offline-candidate
      synthetic A/B pass. Permission denial, delete/reinstall, interrupted staging cleanup,
      corrupt-disable/recovery in the isolated current `.uitest`, backup exclusion, and release
      restart persistence also pass. An exact-reference self-hosted recording reached editable
      review and was cancelled with no destination, fallback, automatic send, or retained app-side
      audio. Real background and force-stopped-process canaries also stop/release AudioRecord and
      cold-start without a draft, review, message, service, or external residue. Distinct exact-once
      current/new sends pass, and the consented 10-item local speaker matrix passes with mean CER
      `0.132922`, p90 `0.241379`, code-switch mean `0.281980`, and zero empty/truncated/signal-warning
      items. A real stopped-upstream canary already proved no local/remote review, fallback, or
      message and restored the home service to healthy. Never record a person merely because ADB
      reconnects, and never invent CER without an exact spoken reference. Retained A/B evidence:
      [`2026-07-19-p6-6-android-asr-ab.json`](docs/evidence/2026-07-19-p6-6-android-asr-ab.json) and
      [`2026-07-19-p6-6-real-speaker-eval.json`](docs/evidence/2026-07-19-p6-6-real-speaker-eval.json).
- [x] **M4:** the owner selected homolab under the hostile-workload/trusted-host-kernel contract.
      Its persistent NixOS service, UID/GID, loopback exposure, exact systemd
      delegation, cgroup exclusivity, native x86_64 execution, representative memory/pids/CPU
      sizing, retained report, persistent switch, and restart stability pass. Hostile-kernel and
      microVM containment are not claimed; see the
      [production evidence](docs/evidence/2026-07-19-m4-production-homolab.md).

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

- Demand-triggered only: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and any future
  second sandbox backend.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.

## Next Execution Order

No committed roadmap milestone remains open. Start only demand-triggered work with a concrete user
or second consumer, while preserving the closed acceptance and safety boundaries above.
