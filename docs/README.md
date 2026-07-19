# TempestMiku — docs

Self-hosted personal AI companion (**Tempest Miku**) in Rust: one persistent character with
swappable capability modes, built on a code-execution agent runtime. A **rewrite** of the running
`hermes-agent` deployment — behavioral **parity first**, then new capabilities.

## Start here

- **[Design docs](design/README.md)** — the full specification, split by section.
- **[Running Miku](running-miku.md)** — local server, retained client contracts, CLI, and e2e run paths.
- **[Coordinator/worker deployment](deploy-coordinator-worker.md)** — environment-neutral build,
  networking, secret-file, supervisor, configuration, isolation, rollout, and recovery contract.
- **[Roadmap](../ROADMAP.md)** — canonical milestones and execution order.
- **[tm-lang T0-T7 closeout evidence](evidence/2026-07-16-tm-lang-closeout.md)** — language/runtime cutover,
  real host/resource/approval adapter, public/Postgres product seam, comparative fluency, and
  default-cutover gates.
- **[tm-lang-only runtime hard cut](evidence/2026-07-16-tm-lang-only-runtime.md)** — sole-language
  runtime contract, removed backend surfaces, preserved behavior, and verification matrix.
- **[P6.6 ASR resumption evidence](evidence/2026-07-18-p6-6-on-device-asr-resumption.md)** — production
  local-default plus explicitly selected self-hosted capture/review design, host inference, signed
  package, passed model activation and airplane-mode inspection, earlier failed speech-quality
  attempts, the latest installed/hash-matched `1.0.3+4` self-hosted-option APK, the same-device
  synthetic A/B, healthy/stopped-upstream self-hosted canaries, lifecycle cleanup, exact current/new
  sends, and the consented 10-item production-local closeout. The privacy-stripped
  [real-speaker aggregate](evidence/2026-07-19-p6-6-real-speaker-eval.json) passes mean/p90/code-switch
  CER and empty/truncation/signal gates without retaining audio or transcript text. The
  [historical deferment note](evidence/2026-07-15-p6-6-on-device-asr-deferment.md)
  remains retained evidence.
- **[P7.2b persona-addenda evidence](evidence/2026-07-18-p7-2b-persona-addenda.md)** — bounded P8-backed
  Auto proposals, manual-only activation/rollback, cross-instance dedupe, public API, and client
  closeout gates.
- **[P9 egress/secret evidence](evidence/2026-07-18-p9-egress-secret-broker.md)** — exact-destination
  HTTPS, pinned DNS/redirect policy, budgets, revocation, opaque handles, replay, and live canary.
- **[P10 MCP runtime evidence](evidence/2026-07-18-p10-mcp-runtime.md)** — selected exact catalog,
  strict schemas/results, untrusted provenance, durable mutations, Streamable HTTP, and official
  read-only live canary through P9.
- **[M4 Linux proc isolation evidence](evidence/2026-07-18-m4-linux-proc-isolation.md)** — executable
  bubblewrap namespace/rlimit and descriptor-pinning canary.
- **[M4 hardened Linux evidence](evidence/2026-07-18-m4-linux-hardened-v1.md)** — fixed sealed seccomp,
  per-run cgroup-v2 enforcement/recovery, disposable Linux/aarch64 plus native x86_64 canaries, strict v1 acceptance
  kit, the explicit hostile-workload/trusted-host-kernel boundary, and live read-only preflights
  showing lumo fails closed without `memory`; the owner-approved homolab native x86_64 bundle is
  retained and locally revalidated.
- **[M4 homolab production evidence](evidence/2026-07-19-m4-production-homolab.md)** — selected native
  x86_64 NixOS target, persistent service, exact systemd/cgroup delegation, representative
  memory/pids/CPU sizing, retained live report, and post-switch restart proof under the accepted
  hostile-workload/trusted-host-kernel boundary.
- **[M4 coordinator/worker deployment evidence](evidence/2026-07-19-m4-coordinator-worker.md)** — one
  authoritative lumo coordinator, one signed durable homolab worker, lumo-owned approval transport,
  idempotent live jobs, fail-no-fallback proof, and the final NixOS namespace compatibility boundary.
- **[P8.1 recall baseline evidence](evidence/2026-07-15-p8-1-recall-baseline.md)** — frozen
  lexical/profile metrics, fixtures, acceptance policy, and versioned record/provenance contracts.
- **[P8.2 durable memory evidence](evidence/2026-07-15-p8-2-durable-memory-spine.md)** — ordered
  scoped storage/migration, correction and unlink denial, readiness, and Postgres gate evidence.
- **[P8.3 hybrid retrieval evidence](evidence/2026-07-15-p8-3-local-embeddings-hybrid.md)** — local
  embedding boundary, pgvector generations, deterministic RRF, and lexical degradation gates.
- **[P8 fuller-memory closeout evidence](evidence/2026-07-15-p8-5-fuller-memory.md)** — turn/resource
  integration, frozen quality metrics, lumo provenance, restart/fallback proof, and final gate matrix.
- **[Changelog](../CHANGELOG.md)** — notable project changes.
- **[Commit message spec](commit-messages.md)** — repository Conventional Commit dialect.

## Design layers

| Layer | Sections | Scope |
|---|---|---|
| **Core runtime** | [`design/core/`](design/core) — §01–16 | The code-execution agent engine: the bet, principles, agent loop, sandbox, host SDK, security, Rust impl, observability. UI / deployment / multi-tenant-agnostic. |
| **Product** | [`design/product/`](design/product) — §20–29 | The companion on top: persona & modes, memory, sub-agents, drive, coding SDK, self-evolution, server & clients, roadmap, parity baseline. Extends the core; does not change the bet (§01). |

Cross-references use section numbers (`§NN` / `§NN.x`) and are **path-independent** — files can move
between folders without breaking them.

Canonical behavioral spec for the rewrite: `SOUL.md` + `skills/` + `honcho.json` + `config.yaml` of
the current deployment — see the **parity baseline** (§29).
