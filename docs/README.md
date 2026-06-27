# TempestMiku — docs

Self-hosted personal AI companion (**Tempest Miku**) in Rust: one persistent character with
swappable capability modes, built on a code-execution agent runtime. A **rewrite** of the running
`hermes-agent` deployment — behavioral **parity first**, then new capabilities.

## Start here

- **[Design docs](design/README.md)** — the full specification, split by section.

## Design layers

| Layer | Sections | Scope |
|---|---|---|
| **Core runtime** | [`design/core/`](design/core) — §01–16 | The code-execution agent engine: the bet, principles, agent loop, sandbox, host SDK, security, Rust impl, observability. UI / deployment / multi-tenant-agnostic. |
| **Product** | [`design/product/`](design/product) — §20–29 | The companion on top: persona & modes, memory, sub-agents, drive, coding SDK, self-evolution, server & clients, roadmap, parity baseline. Extends the core; does not change the bet (§01). |

Cross-references use section numbers (`§NN` / `§NN.x`) and are **path-independent** — files can move
between folders without breaking them.

Canonical behavioral spec for the rewrite: `SOUL.md` + `skills/` + `honcho.json` + `config.yaml` of
the current deployment — see the **parity baseline** (§29).
