# 15. Open questions / risks

- **In-language Python ecosystem.** If real Python libraries become necessary, expose narrow host
  capabilities or a separately isolated service. Do not add a second language or ambient package
  runtime without an explicit owner decision.
- **tm startup/session cost.** Measure real session pressure before adding pooling or snapshots.
- **Streaming a *running* cell's stdout to the model** (not just the UI) — deferred; assistant
  tokens and tool-call args stream from day 1 (§5.5), but the model still sees one shaped result
  per finished cell.
- **Long-running / background tasks** inside a cell — needs a job model; out of scope for v1.
- **Artifact token budgeting** — preview sizes and slice defaults need tuning against real usage.
- **Multi-tenant isolation** — one runtime session per run is fine single-user; shared deployment needs
  per-tenant capability + resource partitioning.
- **External read-through resource schemes** — `issue://` / `pr://` (GitHub) and `mcp://` (MCP server
  *resources*, §25.1) are **parked**. Unlike the internal schemes (§9.3) they are network-egress
  proxies needing the allowlist (§08), a secret-broker credential (§08.3), and a disk cache; they
  register into the same §9.2 resolver behind the same handler contract when enabled.
