# TempestMiku — delivery history

> The historical record of **how** TempestMiku was built: the milestone roadmap and the dated
> acceptance evidence behind each closed capability. It is provenance, not the entry point for new
> work.
>
> - **What the product is** (as-built, present tense): [`../PRODUCT.md`](../PRODUCT.md).
> - **The detailed specification** (section-numbered): [`../design/README.md`](../design/README.md).
> - **This layer** captures the build sequence and proof.

## Milestone roadmap

- [**ROADMAP.md**](ROADMAP.md) — the ordered core (M0–M4) and product (P0a–P11) milestone plan,
  per-milestone acceptance checks, the execution sequence, the deferred-work registry, and the
  integrated verification matrix. Every listed milestone is closed. Design-doc `§NN` references in it
  remain stable.
- [**TODO.md**](TODO.md) — the closed executable checklist and regression invariants that
  accompanied that roadmap. It is retained for provenance and has no open work items.

The build was dogfooded in the order **coding agent → project manager → personal assistant**: the
first useful slice helped build TempestMiku itself, then managed the project around that work, then
broadened into the full companion. Behavioral parity with the source `hermes-agent` deployment (§29)
came before any new capability layered on.

## Acceptance evidence

Machine-readable and narrative acceptance evidence is retained under [`../evidence/`](../evidence).
The filenames are dated and milestone-tagged on purpose — they are immutable proof captured at
closeout and are not rewritten. Grouped by area:

### Language & runtime cutover

- [`tm-lang T0–T7 closeout`](../evidence/2026-07-16-tm-lang-closeout.md) — language/runtime cutover,
  host/resource/approval adapter, public/Postgres product seam, comparative fluency, default cutover.
- [`tm-lang-only runtime hard cut`](../evidence/2026-07-16-tm-lang-only-runtime.md) — sole-language
  runtime contract, removed backend surfaces, preserved behavior, verification matrix.
- [`tm-conformance-v2 batch hardening`](../evidence/2026-07-17-tm-conformance-v2-batch-hardening.md) —
  semicolon-cell hard cut and scope-aware forward-binding batch execution.
- [`tm fluency prompt v2`](../evidence/2026-07-16-tm-fluency-prompt-v2/) — the frozen comparative
  fluency corpus, summary, and raw run outputs.

### Persona & voice

- [`P2 voice dialectic`](../evidence/2026-07-18-p2-voice-dialectic.md) — bounded dialectic cadence,
  frozen voice calibration, and the live-report contract.
- [`P2 live voice`](../evidence/2026-07-18-p2-voice-live.json) — the retained non-echo `gpt-5.5`
  General/grounding/Serious rubric run.

### Coding backend

- [`P0a OMP ACP live`](../evidence/2026-07-18-p0a-omp-acp-live.json) — real-model OMP ACP dogfood:
  public approval, exact edit, targeted test, artifact/final events, and `Last-Event-ID` replay.

### Memory

- [`P8.1 recall baseline`](../evidence/2026-07-15-p8-1-recall-baseline.md) — frozen lexical/profile
  metrics, fixtures, acceptance policy, and record/provenance contracts
  ([lexical baseline JSON](../evidence/2026-07-15-p8-1-lexical-baseline.json)).
- [`P8.2 durable memory spine`](../evidence/2026-07-15-p8-2-durable-memory-spine.md) — ordered scoped
  storage/migration, correction and unlink denial, readiness, and Postgres gates.
- [`P8.3 local embeddings / hybrid`](../evidence/2026-07-15-p8-3-local-embeddings-hybrid.md) — local
  embedding boundary, pgvector generations, deterministic RRF, and lexical degradation gates.
- [`P8 fuller-memory closeout`](../evidence/2026-07-15-p8-5-fuller-memory.md) — turn/resource
  integration, frozen quality metrics, lumo provenance, restart/fallback, and the final gate matrix
  ([closeout JSON](../evidence/2026-07-15-p8-5-fuller-memory.json)).

### Self-evolution

- [`P7.2b persona addenda`](../evidence/2026-07-18-p7-2b-persona-addenda.md) — bounded P8-backed Auto
  proposals, manual-only activation/rollback, cross-instance dedupe, and public API/client closeout.

### Egress & MCP

- [`P9 egress / secret broker`](../evidence/2026-07-18-p9-egress-secret-broker.md) —
  exact-destination HTTPS, pinned DNS/redirect policy, budgets, revocation, opaque handles, replay,
  and live canary.
- [`P10 MCP runtime`](../evidence/2026-07-18-p10-mcp-runtime.md) — selected exact catalog, strict
  schemas/results, untrusted provenance, durable mutations, Streamable HTTP, and official read-only
  live canary through P9.

### Runtime isolation & deployment (M4)

- [`M4 Linux proc isolation`](../evidence/2026-07-18-m4-linux-proc-isolation.md) — bubblewrap
  namespace/rlimit and descriptor-pinning canary.
- [`M4 hardened Linux v1`](../evidence/2026-07-18-m4-linux-hardened-v1.md) — sealed seccomp, per-run
  cgroup-v2 enforcement/recovery, disposable and native x86_64 canaries, acceptance kit, and the
  explicit hostile-workload/trusted-host-kernel boundary.
- [`M4 homolab production`](../evidence/2026-07-19-m4-production-homolab.md) — the selected native
  x86_64 NixOS service: exact systemd/cgroup delegation, sizing, retained live report, and restart
  proof ([native report JSON](../evidence/2026-07-19-m4-production-homolab-x86_64.json)).
- [`M4 coordinator / worker`](../evidence/2026-07-19-m4-coordinator-worker.md) — one authoritative
  lumo coordinator, one signed durable homolab worker, lumo-owned approvals, idempotent jobs, and the
  fail-no-fallback boundary.

### Android & on-device voice (P6)

- [`P6.6 on-device ASR deferment`](../evidence/2026-07-15-p6-6-on-device-asr-deferment.md) — the
  historical SenseVoice feasibility run and its limitations.
- [`P6.6 on-device ASR resumption`](../evidence/2026-07-18-p6-6-on-device-asr-resumption.md) — the
  production local-default plus explicitly selected self-hosted capture/review design, host
  inference, signed packaging, model activation, airplane-mode cold start, same-device A/B, lifecycle,
  and current/new-session sends.
- [`P6.6 real-speaker evaluation`](../evidence/2026-07-19-p6-6-real-speaker-eval.json) — the
  privacy-stripped consented 10-item production-local corpus aggregate (audio/transcript not
  retained), alongside the [Android A/B](../evidence/2026-07-19-p6-6-android-asr-ab.json) and
  [streaming-Paraformer host eval](../evidence/2026-07-18-p6-6-streaming-paraformer-host-eval.json).

Additional M4 preflight/contract sidecars (`*-readonly-preflight.json`, `*-disposable-native-*`) are
retained beside the reports above for exact reproduction.
