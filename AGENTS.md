# TempestMiku agent guide

## Project identity

- TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: self-hosted, single-user, characterful personal AI companion plus code-execution agent runtime.
- Parity comes first. P0-P3 are not done until they preserve the current behavior for their slice: Miku voice, mode router, memory recall/profile, and manual approvals.
- This is not a generic coding agent. Product work must preserve the constant Miku identity and the serious-mode voice cap.

## Current workspace state

- Rust workspace, edition 2024. Members are discovered by `crates/*` and `apps/*`.
- Existing crates/apps:
  - `crates/tm-core`: message types, streaming agent loop, result shaping, `LlmClient`, `Sandbox`, `EventSink` traits.
  - `crates/tm-llm`: OpenAI-compatible streaming SSE client.
  - `crates/tm-sandbox`: current M0 stub sandbox only; real `deno_core` backend is still future M1 work.
  - `apps/tm-cli`: CLI wiring the LLM client, agent loop, and stub sandbox.
- Planned but not yet materialized: `tm-host`, `tm-artifacts`, `tm-mcp`, `tm-trace`, `tm-server`, `tm-persona`, `tm-memory`, `tm-agents`, `tm-drive`, WebUI, Android.

## Read before changing code

Start with the narrowest docs for the task:

- Overview: `docs/README.md`, `docs/design/README.md`.
- Core architecture: `docs/design/core/01-tldr-the-bet.md`, `03-core-principles.md`, `04-architecture.md`, `05-agent-loop.md`, `06-repl-sandbox.md`, `07-host-sdk.md`, `08-security-model.md`, `09-context-artifacts.md`, `10-rust-implementation.md`, `14-roadmap.md`.
- Product layer: `docs/design/product/20-product-overview.md`, `21-persona-and-modes.md`, `22-memory-dreaming.md`, `23-agents-orchestration.md`, `24-drive-storage.md`, `25-coding-agent-sdk.md`, `26-self-evolution.md`, `27-server-and-clients.md`, `28-product-roadmap.md`, `29-parity-baseline.md`.
- Roadmap execution order lives in `docs/design/product/28-product-roadmap.md` under “Execution plan from current workspace”. Follow it unless the user explicitly changes priority.

## Architectural invariants

- One model-visible tool: `execute(code)`. Capabilities are SDK namespaces the code calls, not chat-native tool sprawl.
- Streaming is the source of truth. `LlmClient::chat_stream` drives the loop; non-streaming chat is just drain + assemble.
- The agent loop owns message accumulation, tool-call execution, result shaping, and `EventSink` callbacks. Do not fork a second loop for product features.
- Sandbox backends sit behind `Sandbox` / `Session`. Product code must not depend on the current stub behavior.
- Capabilities are config and grants, not hardcoded privilege. Unknown capability/resource schemes fail closed.
- Real-repo reach is curated: `proc.run(cmd, args)` with argv-vector execution, allowlist, linked cwd, and approval. Never add a raw shell escape hatch.
- Artifacts and large outputs should spill to the artifact store. Do not push large tool/sub-agent output into model context.
- Product features extend the core; they do not weaken the core bet or bypass approval/security boundaries.

## Milestone sequencing

Default next work:

1. Close M0 with regression tests around streaming, tool-call assembly, stub eval, and CLI token streaming.
2. Add `tm-artifacts` before broad SDK/agent/server work, because spill + `artifact://` are shared infrastructure.
3. Add the real `deno_core` sandbox before product dogfooding; avoid a special chat-only P0 path.
4. Add `tm-host` + `ApprovalPolicy` before linked-folder editing or `proc.run`.
5. Ship P0 as a vertical slice: `tm-server` SSE + persona overlay + minimal memory recall/profile together.

Do not start yet unless explicitly requested:

- Android before P0/P1 server APIs stabilize.
- Drive auto-organizer before approval policy and memory scopes exist.
- Self-evolution writes before audit/replay and tier gates exist.
- Deep research/orchestration before `agents.*`, artifacts, memory scopes, and scheduler are real.

## Coding conventions

- Prefer small, boring Rust modules with explicit boundaries. Do not introduce framework abstractions before two concrete users exist.
- Keep traits at crate boundaries (`tm-core`, `tm-host`) and concrete implementations in satellite crates.
- Use `thiserror` inside crates and `anyhow` at binary edges.
- Use `serde` for wire/config/op boundaries; keep host SDK boundaries `serde_json::Value` where docs specify late-bound capability calls.
- Preserve streaming-first behavior in tests: assert deltas/events, not only final strings.
- For exported symbol changes, update every caller and tests in the same change. No compatibility shims unless the user asks.

## Testing and verification

- Baseline command: `cargo test`.
- Add narrow tests for the behavior you change before relying on the full suite.
- For streaming changes, use scripted streams; do not require network.
- Live OpenAI tests must stay opt-in/gated by environment and must not run in normal `cargo test`.
- For approval/security work, test the denial path, timeout/default-deny behavior, and fail-closed unknown capability/resource behavior.
- For sandbox work, test persistence across cells, reset, timeout/cancel, output caps, display/artifact capture, and blocked network/filesystem access.

## Documentation rules

- Keep design docs and implementation aligned. If behavior changes milestone scope, update the relevant `docs/design/**` section in the same PR/change.
- Cross-references use section numbers (`§NN`) and should remain path-independent.
- Do not mark a roadmap stage done until its acceptance checks pass.
