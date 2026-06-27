# 14. Roadmap

| Milestone | Deliverable |
|---|---|
| **M0 — streaming protocol skeleton** | Streaming-first OpenAI client (SSE) + the agent loop consuming deltas live + an `EventSink` (assistant tokens token-by-token) + a stub sandbox (echoes code). Validates the streaming message protocol end-to-end; tests drive a scripted stream with no network. |
| **M1 — real REPL** | `deno_core` backend; ops for `display`, `http.get` (allowlisted), `artifacts`; result-shaping + artifact spill; system prompt. |
| **M2 — disclosure + tools** | `tools.search/docs/call`; `tm-mcp` imports external MCP tools into the catalog; secrets-by-reference. |
| **M3 — production loop** | Approval gates, audit log, session pooling, deterministic replay. (Streaming landed in M0.) |
| **M4 — backends & hardening** | `rquickjs` + Python-subprocess backends; Linux isolation (seccomp/cgroups/microVM); egress hardening. |
