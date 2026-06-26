# 15. Open questions / risks

- **In-language Python ecosystem.** Decided: TS on `deno_core`. If real Python libraries become
  necessary in-REPL, the CPython-subprocess backend (§6.6) drops in behind the `Sandbox` trait —
  loop, SDK, and registry unchanged.
- **V8 startup cost.** Mitigated by session pooling; measure before optimizing.
- **Streaming a *running* cell's stdout to the model** (not just the UI) — deferred; assistant
  tokens and tool-call args stream from day 1 (§5.5), but the model still sees one shaped result
  per finished cell.
- **Long-running / background tasks** inside a cell — needs a job model; out of scope for v1.
- **Artifact token budgeting** — preview sizes and slice defaults need tuning against real usage.
- **Multi-tenant isolation** — one isolate per run is fine single-user; shared deployment needs
  per-tenant capability + resource partitioning.
