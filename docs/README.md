# TempestMiku — docs

Self-hosted personal AI companion (**Tempest Miku**) in Rust: one persistent character with
swappable capability modes, built on a code-execution agent runtime. A **rewrite** of the running
`hermes-agent` deployment — behavioral **parity first**, then new capabilities.

## Start here

- **[Design docs](design/README.md)** — the full specification, split by section.
- **[Running Miku](running-miku.md)** — local server, browser app, CLI, and e2e run paths.
- **[Roadmap](../ROADMAP.md)** — canonical milestones and execution order.
- **[P6.6 ASR deferment evidence](evidence/2026-07-15-p6-6-on-device-asr-deferment.md)** — retained
  benchmark, Taiwan Mandarin findings, open gates, and resume contract.
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
