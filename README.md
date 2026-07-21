# Tempest Miku

TempestMiku is a self-hosted, single-user personal AI companion in Rust: one persistent character
with General and Serious Engineer capability modes, durable memory, project continuity, manual
approvals, and a streaming code-execution runtime.

The defining bet is one model-visible tool — `execute(code)`. The model writes persistent `tm` code;
that code discovers and calls capability-scoped SDK namespaces for resources, files, processes,
memory, agents, drive, egress, and selected MCP imports. Large outputs remain behind artifact and
resource references instead of flooding the model context.

`tm-server` is the authoritative API and security boundary. It owns durable sessions and turns,
replayable SSE, project and memory authority, approvals, scheduling, and the product subsystems. One
Flutter codebase provides Web/PWA and Android clients for conversation, history, projects, scoped
Drive/resources, reviewed changes, modes, notifications, imports, and editable voice drafts.

Production is single-owner and self-hosted: loopback behind HTTPS/Tailscale Serve, Postgres for
durable deployments, exact per-turn grants, default-disabled destination-scoped egress, opaque
secrets, argv-vector `proc.run`, and fail-closed linked-folder policy. An optional signed `tm-worker`
exposes a linked homolab host without creating a second session, model, memory, or approval authority.

## Product boundaries

- Constant Miku identity; serious work lowers voice intensity but never replaces the character.
- No raw shell, ambient filesystem/network authority, chat-native tool sprawl, or silent approval.
- No automatic persona activation, raw `SOUL.md` mutation, aggressive self-rewrite, Firebase/FCM,
  silent ASR fallback, or automatic voice send.
- Cloud sync/CRDT, AST/LSP SDK surfaces, `tm-trace`, microVM isolation, and extra sandbox backends are
  demand-triggered rather than implied by the current product.

## Project docs

- [Final product document](docs/PRODUCT.md)
- [Section-numbered specification](docs/design/README.md)
- [Running Miku](docs/running-miku.md)
- [Coordinator/worker deployment](docs/deploy-coordinator-worker.md)
- [Delivery history and acceptance evidence](docs/history/README.md)
- [Changelog](CHANGELOG.md)
- [Commit message spec](docs/commit-messages.md)
- [LLM-to-Miku E2E hatch](apps/tm-e2e/README.md)
