# TODO

Last aligned: **2026-07-15**.

Active milestone: **P8 — fuller memory**.

Active slice: **P8.1 — replayable recall baseline and record contracts**.

`ROADMAP.md` is the milestone-order and completion-history source. This file contains the ordered
P8 execution slices, their regression boundaries, and explicit deferments; only P8.1 is active now.

P6.6 on-device ASR was explicitly deferred on 2026-07-15. Its implementation state, model and
corpus provenance, physical results, quality gaps, raw artifact hashes, and resume contract live in
[`docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md`](docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md).
P6.1–P6.5 remain closed; neither P6.6 nor P6 is marked complete.

## P8 Outcome

TempestMiku can measure the current lexical/profile recall baseline, persist richer scoped memory
records with evidence and correction history, and retrieve bounded hybrid lexical/dense candidates
without weakening server-owned owner/scope authority. Dense recall is self-hosted and replaceable;
when it is disabled or unavailable, recall degrades visibly to the existing lexical path.

## P8.1 — Replayable Baseline And Contracts

- [ ] Freeze replayable evaluation fixtures for global and `project:<slug>` recall. Include relevant,
      irrelevant, unsupported, stale, corrected, superseded, and cross-scope cases; record the
      current Postgres FTS/profile baseline before changing ranking.
- [ ] Check in a versioned evaluator and manifest before tuning. Report nDCG@5, Recall@5, unsupported
      or stale precision failures, scope leaks, prompt tokens, candidate counts, and p50/p95 latency;
      keep a held-out fixture split and the existing 1,600-token/five-recall-item final bounds.
- [ ] Freeze the P8 acceptance policy: hybrid nDCG@5 improves at least 10% relative to the lexical
      baseline without reducing Recall@5; corrected, superseded, unsupported, and cross-scope items
      have zero false inclusions; the benchmark-derived latency ceiling is recorded before tuning.
- [ ] Add versioned episodic and semantic record shapes with server-owned `owner_subject` and
      `memory_scope`, source session/event or resource evidence, confidence, observed/effective
      timestamps, status, and correction/supersession links.
- [ ] Freeze an embedding provenance contract covering provider/model id, dimensions, normalization,
      content hash, embedding version, created time, and deterministic re-embedding state.
- [ ] Exit P8.1 only when the lexical baseline artifact is reproducible from a clean database and
      the contracts compile across `tm-memory`, both stores, and serialized resource/evidence shapes.

## P8.2 — Durable Scoped Memory Spine

- [ ] Add ordered Postgres migrations for episodic/semantic records, evidence, correction history,
      embedding jobs/provenance, and idempotent content/version keys; migrate existing profile facts,
      summaries, and recall chunks without changing their lexical results.
- [ ] Enforce subject/scope filtering in storage queries and correction/supersession filtering before
      ranking. Project unlink tombstones revoke reads and queued embedding work immediately.
- [ ] Add startup/readiness states that distinguish missing pgvector, unsupported dimensions, corrupt
      schema, and temporarily unavailable embeddings. Durable roles never fall back to process-local
      authority or writes.
- [ ] Exit P8.2 with clean-schema, upgrade, restart, rollback-safe failure, idempotent replay, and
      cross-subject/cross-scope denial coverage in gated Postgres tests.

## P8.3 — Local Embeddings And Hybrid Retrieval

- [ ] Add a narrow batch-capable embedding boundary with `disabled` and self-hosted `local` modes;
      pin model id, dimensions, normalization, timeout, batch/input limits, and content/version hash.
      `openai_compatible` remains optional and cannot be required for production acceptance.
- [ ] Add pgvector storage/index creation plus resumable, deduplicated re-embedding. A provider/model/
      dimension change creates a new version and atomically switches only after coverage is complete.
- [ ] Generate lexical and dense candidates independently, fuse them with a deterministic bounded
      RRF policy, and preserve rank/score components, evidence, subject, scope, and embedding version.
- [ ] Apply correction/supersession and unsupported-fact filtering before context budgeting. LLM
      extraction may propose candidates but cannot silently promote inference into owner truth.
- [ ] Exit P8.3 when disabled, provider-loss, partial re-embedding, stale-version, and dimension/model
      mismatch cases visibly use lexical-only recall with identical subject/scope enforcement.

## P8.4 — Server Integration And Inspectability

- [ ] Route hybrid results through the existing `MemoryProvider`, prompt budgeter, and turn assembly;
      do not create a second agent loop, reorder durable turn semantics, or add ambient `memory.*`
      authority.
- [ ] Extend exact `memory://` resources and public SDK/types only as needed to inspect provenance,
      correction status, score components, embedding version, degraded mode, and budget decisions;
      previews remain bounded and authority-filtered.
- [ ] Feed approved dream/extraction output into typed candidate records with evidence and idempotent
      source keys. Unsupported inference stays reviewable/withheld and never becomes owner truth.
- [ ] Exit P8.4 with scripted turn/resource tests proving the same bounded result after restart,
      provider loss, correction, project unlink, and exact event replay.

## P8.5 — Quality Gate And Closeout

- [ ] Unit tests cover fusion determinism, deduplication, budget truncation, score ties, stale and
      corrected records, unsupported claims, provider/model/dimension mismatch, and disabled/local
      degradation.
- [ ] Gated Postgres tests cover migrations, vector/lexical parity, subject and scope isolation,
      unlink tombstones, restart persistence, re-embedding, idempotent replay, and fail-closed
      unknown records/configuration.
- [ ] Replay the frozen evaluation set against lexical-only and hybrid recall. Record machine-readable
      evidence for both fixture splits showing the frozen relevance threshold, zero false inclusions,
      bounded context/latency, and truthful provider-loss degradation.
- [ ] Record self-hosted lumo evidence for local model provenance/hash, pgvector capability, cold and
      warm latency, provider restart/loss, resumable re-embedding, server restart, and lexical-only
      operation with external network access unavailable.
- [ ] Run Rust format/clippy/tests, affected server/e2e suites, clean-schema and restart Postgres
      gates, and client checks for any surfaced provenance/resource changes.
- [ ] Align §22, public SDK/resource contracts, `ROADMAP.md`, `TODO.md`, and `AGENTS.md`; do not mark
      P8 complete until the replayable quality evidence exists.

## Regression Invariants

- Preserve Miku identity, mode routing, streaming-first turns, durable SSE replay, exact capability
  grants, manual approvals, and server-owned owner/scope authority.
- Preserve the current lexical/profile/summaries path as the safe degraded mode. Embedding failure
  may reduce recall quality but must not broaden scope, invent facts, or make durable roles silently
  non-durable.
- Keep memory evidence inspectable and correctable. Retrieval scores and LLM extraction are
  heuristics, never owner truth without supported provenance.
- Keep the deployment self-hosted on the existing Postgres spine; do not add an external memory SaaS
  dependency or expose secret material to model context.

## Explicit Deferments

- **P6.6 on-device ASR:** deferred until the owner explicitly reopens it and the independent evidence
  note's text-quality, real-speech, memory-headroom, production-flow, and signed-device resume gates
  are accepted. Do not add production microphone permission, weights, model installer, ASR runtime,
  server/cloud fallback, or automatic send while deferred.
- **P7.2b persona addenda:** waits for P8 provenance, correction, scope, and recall-quality gates.
  Auto mode may eventually propose; it never approves or activates persona changes.
- **P9/P10:** production egress and opaque-secret hardening precede MCP or live external research.
- Demand-triggered only: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and additional
  sandbox backends.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.

## Queued After P8

1. P7.2b Auto-mode persona proposals with durable manual activation and rollback.
2. P9 production egress and opaque secret broker.
3. P10 MCP catalog and live research through the P9 boundary.

P6.6 has no scheduled queue position while deferred.
