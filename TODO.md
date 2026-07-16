# TODO

Last aligned: **2026-07-15**.

Active milestone: **P7.2b — approval-backed persona addenda**.

`ROADMAP.md` is the milestone-order and completion-history source. P8 is closed; its implementation,
quality metrics, lumo provenance, restart/fallback evidence, and exact gate matrix live in
[`docs/evidence/2026-07-15-p8-5-fuller-memory.md`](docs/evidence/2026-07-15-p8-5-fuller-memory.md).

P6.6 on-device ASR remains explicitly deferred. Preserve its interface spike and evidence, but do
not add microphone permission, production weights/runtime, server fallback, or automatic send unless
the owner explicitly resumes it.

## P7.2b Outcome

Auto mode may notice repeated, evidence-backed persona preference mismatches and propose a bounded
tone/address/interaction addendum. It never applies or rolls back persona guidance automatically.
Only a durable manual approval may activate an immutable version, and no addendum may change Miku's
identity, `SOUL.md`, safety rules, modes, voice caps, skills, scopes, or capabilities.

## P7.2b — Approval-Backed Persona Addenda

- [ ] Freeze the typed addendum/evidence contract, stable-base digest, dedupe key, cooldown, and
      conservative Auto-mode proposal threshold. Unsupported inference remains withheld.
- [ ] Add immutable version storage plus exact review, active-version, history, and rollback resource
      views behind existing server-owned authority and durable approvals.
- [ ] Compose only approved tone/address/interaction guidance on the next turn. Deny, timeout, stale
      base, retry, restart, and rollback must remain replayable and fail closed.
- [ ] Prove auto-propose/never-auto-apply, bounded evidence, idempotency, next-turn composition,
      rollback, and invariance of identity, authority, `SOUL.md`, safety, modes, and capabilities.
- [ ] Run strict Rust, gated Postgres/restart, e2e, and affected client gates; align §21/§26, SDK,
      `ROADMAP.md`, `TODO.md`, and `AGENTS.md` before closing P7.2b.

## Regression Invariants

- Preserve Miku identity, mode routing, streaming-first turns, durable SSE replay, exact grants,
  manual approvals, and server-owned owner/scope authority.
- Preserve P8's bounded hybrid recall, inspectable evidence/corrections, exact retry context, and
  truthful lexical fallback. Retrieval scores and model inference are never owner truth.
- Keep persona proposals narrow and reversible; never mutate `SOUL.md` or widen tools, capabilities,
  resources, scopes, safety rules, or deployment authority.

## Explicit Deferments

- **P6.6 on-device ASR:** deferred until the owner explicitly reopens the independent evidence note's
  quality, memory-headroom, production-flow, and signed-device resume gates.
- **P9/P10:** production egress and opaque-secret hardening precede MCP or live external research.
- Demand-triggered only: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and additional
  sandbox backends.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.

## Queued After P7.2b

1. P9 production egress and opaque secret broker.
2. P10 MCP catalog and live research through the P9 boundary.

P6.6 has no scheduled queue position while deferred.
