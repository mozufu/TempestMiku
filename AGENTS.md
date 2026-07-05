# TempestMiku agent guide

## Project identity

- TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: self-hosted, single-user, characterful personal AI companion plus code-execution agent runtime.
- Parity comes first. Shipped P0-P3 slices must keep preserving current behavior: Miku voice, mode router, memory recall/profile, and manual approvals.
- This is not a generic coding agent. Product work must preserve the constant Miku identity and the serious-mode voice cap.

## Current workspace state

- Rust workspace, edition 2024. Members are discovered by `crates/*` and `apps/*`.
- Existing crates/apps:
  - `crates/tm-core`: message types, streaming agent loop, result shaping, `LlmClient`, `Sandbox`, `EventSink` traits.
  - `crates/tm-llm`: OpenAI-compatible streaming SSE client.
  - `crates/tm-sandbox`: M0 `StubSandbox` plus the M1 `deno_core` JS/TS backend with persistent cells, timeout/reset, output spill, resource reads, SDK prelude, and host-call bridge.
  - `crates/tm-artifacts`: session artifacts and content-addressed blob storage.
  - `crates/tm-host`: host registry, capability grants, approval policy, resource registry, linked folders, `fs.*`, `code.*`, and argv-vector `proc.run`.
  - `crates/tm-modes`: mode catalog, routing, voice caps, and configurable mode/skill asset status.
  - `crates/tm-agents`: actor lifecycle, mailbox, `agents.run/spawn/parallel/msg/send/wait/inbox/list`, supervision defaults, and `agent://` / `history://` resources.
  - `crates/tm-server`: axum session API, replayable SSE/event store, minimal memory provider, approvals, artifact routes, Serious Engineer backend path, and OMP ACP bridge.
  - `apps/tm-cli`: CLI wiring the LLM client, streaming agent loop, and Deno sandbox by default; `--stub-sandbox` remains for protocol tests.
  - `apps/tm-e2e`: local/dev public-API workflow harness for scripted and opt-in live session smoke coverage.
  - `clients/miku_flutter` / `clients/miku_web`: client scaffolds and smoke coverage.
- Not production-complete or not yet split into dedicated crates: `tm-mcp`, `tm-trace`, full `tm-memory`, `tm-drive`, scheduler/dreaming, remaining P3+ actor surface (`broadcast`/`pipeline`, restart/cancel, DAG/protocol enforcement, fuller provenance), richer drive/skill/memory approval surfaces, and Android OS packaging.

## Read before changing code

Start with the narrowest docs for the task:

- Overview: `docs/README.md`, `docs/design/README.md`.
- Core architecture: `docs/design/core/01-tldr-the-bet.md`, `03-core-principles.md`, `04-architecture.md`, `05-agent-loop.md`, `06-repl-sandbox.md`, `07-host-sdk.md`, `08-security-model.md`, `09-context-artifacts.md`, `10-rust-implementation.md`; core roadmap lives in `ROADMAP.md` (§14).
- Product layer: `docs/design/product/20-product-overview.md`, `21-persona-and-modes.md`, `22-memory-dreaming.md`, `23-agents-orchestration.md`, `24-drive-storage.md`, `25-coding-agent-sdk.md`, `26-self-evolution.md`, `27-server-and-clients.md`, `29-parity-baseline.md`; product roadmap lives in `ROADMAP.md` (§28).
- Roadmap execution order lives in `ROADMAP.md` under “Execution plan from current workspace”. Follow it unless the user explicitly changes priority.

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

1. Continue P3+ actor work: `broadcast`/`pipeline`, restart strategies, cancel tokens, DAG/protocol enforcement, and fuller provenance.
2. Keep the native/OMP coding backend boundary boring: OMP ACP remains replaceable, while native Deno remains the dogfood path for `fs.*` / `code.*` / `proc.*`, artifacts, and HTTP-routed manual approvals.
3. Defer expansion crates and surfaces (`tm-memory`, `tm-drive`, `tm-mcp`, `tm-trace`, scheduler/dreaming, Android OS packaging) until the P3+ actor and server/resource surfaces are stable.

Do not start yet unless explicitly requested:

- Android OS packaging before the P2/P3 product surfaces stabilize.
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

- Use the project dev shell for repo toolchains: run commands as `nix develop --command <cmd>` when a tool may be provided by Nix, especially Flutter/Dart/client commands or anything missing from the ambient PATH. Prefer this non-interactive form over entering a long-running `nix develop` shell.
- Baseline command: `cargo test`.
- Add narrow tests for the behavior you change before relying on the full suite.
- For streaming changes, use scripted streams; do not require network.
- Live OpenAI tests must stay opt-in/gated by environment and must not run in normal `cargo test`.
- For approval/security work, test the denial path, timeout/default-deny behavior, and fail-closed unknown capability/resource behavior.
- For sandbox work, test persistence across cells, reset, timeout/cancel, output caps, display/artifact capture, and blocked network/filesystem access.

## Commit messages

- Use the repository Conventional Commit dialect in `docs/commit-messages.md`: `<type>(<machine>/<scope>): <subject>`.
- Prefer stable machine/scope pairs from the spec, for example `core/agent-loop`, `sandbox/deno`, `host/approval`, `server/sse`, `web/approvals`, `docs/roadmap`, or `repo/workspace`.
- Split unrelated intents into separate commits; keep tests, callsite migrations, and required docs with the behavior they prove or describe.

## Documentation rules

- Keep design docs and implementation aligned. If behavior changes milestone scope, update the relevant `docs/design/**` section in the same PR/change.
- Cross-references use section numbers (`§NN`) and should remain path-independent.
- Do not mark a roadmap stage done until its acceptance checks pass.
