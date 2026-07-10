# 4. Architecture

```mermaid
flowchart TD
    U[User] --> O[Orchestrator / agent loop]
    O <-->|chat completions| L[LlmClient -- OpenAI-compatible]
    O -->|execute code| S[Sandbox session]
    S -->|host calls| H[Host capability registry]
    H --> NET[HTTP egress -- allowlist]
    H --> FSY[Filesystem -- root-jailed]
    H -.future.-> MCP[tm-mcp -- deferred]
    H -.future.-> SEC[Secret broker -- deferred]
    S --> ART[Artifact store]
    O --> ART
    S -->|read uri| RR[Resource resolver registry]
    RR --> ART
    O -.future crate.-> TR[tm-trace -- deferred]
    H -.future crate.-> TR
```

Components:

- **Orchestrator** — owns the message list, runs the loop, applies the result-shaping policy,
  enforces turn/budget limits.
- **LlmClient** — talks to the OpenAI-compatible endpoint; **streaming-first** (SSE), with a non-streaming convenience that drains the stream.
- **Sandbox / Session** — executes code cells in a persistent, isolated environment.
- **Host capability registry** — the set of Rust-backed functions the SDK exposes to code.
- **Artifact store** — content-addressed storage for large outputs; hands back `artifact://` refs.
- **Resource resolver registry** — one scheme-dispatched `read(uri)` over registered handlers
  (currently `artifact://` / `workspace://session` / `linked://` / `project://` / `memory://` /
  `agent://` / `history://` / `cron://` and configured `drive://`; `skill://` reads remain reserved
  until their owning milestone registers a handler and grants); §9.2.
- **Secret broker** — deferred P7 surface for resolving opaque handles only at the host boundary.
- **Transcript / tracing** — current structured events/artifacts provide audit evidence; a dedicated
  `tm-trace` replay crate remains deferred.

The product server (§27) wraps these components with authenticated durable turns and one replayable
SSE envelope. Postgres-backed `api`, `worker`, and `all` roles own persistence and supervision; they
do not create a second agent loop. A Deno session remains thread-affine to its runtime shard while the
core loop continues to own accumulation, execution, and result shaping.
