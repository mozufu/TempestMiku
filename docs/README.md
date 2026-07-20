# TempestMiku — documentation

TempestMiku is a self-hosted, single-user personal AI companion in Rust: one persistent Miku identity over General, Serious Engineer, and Handoff workflows, built on a streaming code-execution runtime.

## Start here

1. **[Final product document](PRODUCT.md)** — the present-tense, as-built description of the complete product.
2. **[Section-numbered specification](design/README.md)** — the detailed core (§01–16), product (§20–30), and `tm` language contracts.
3. **[Running Miku](running-miku.md)** — server, Flutter clients, CLI, and E2E run paths.
4. **[Coordinator/worker deployment](deploy-coordinator-worker.md)** — build, networking, secrets, supervision, isolation, rollout, and recovery.
5. **[Delivery history](history/README.md)** — the archived milestone roadmap and dated acceptance evidence. Use this for provenance, not as the product entrypoint.

## Specification layers

| Layer | Sections | Scope |
|---|---|---|
| **Core runtime** | [`design/core/`](design/core) — §01–16 | One `execute(code)` tool, streaming agent loop, tm sandbox, capability registry, security, artifacts/resources, OpenAI-compatible transport, observability. |
| **Product** | [`design/product/`](design/product) — §20–30 | Persona and modes, memory, actors, projects, drive, coding SDK, bounded evolution, server, scheduler, Flutter/Web/Android clients, parity contract. |
| **Language** | [`design/tm/`](design/tm) | The frozen `tm-conformance-v2` syntax, effects, approval suspension, persistent REPL, data pipelines, and presentation trace. |

Cross-references use stable section numbers (`§NN` / `§NN.x`). The source behavioral contract is the original `SOUL.md` + `skills/` + `honcho.json` + `config.yaml`, pinned in §29.

## Other references

- [Changelog](../CHANGELOG.md)
- [Commit message specification](commit-messages.md)
- [Acceptance evidence directory](evidence/)
