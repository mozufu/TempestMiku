# TempestMiku agent guide

## Project identity

- TempestMiku is a self-hosted, single-user, characterful personal AI companion plus code-execution
  agent runtime, implemented in Rust.
- The canonical product contract is [`docs/PRODUCT.md`](docs/PRODUCT.md), backed by the
  section-numbered specification under `docs/design/`. The source `hermes-agent` behavior pinned in
  §29 remains the parity baseline; delivery stages are historical provenance, not an active plan.
- This is not a generic coding agent. Product work must preserve the constant Miku identity and the
  serious-mode voice cap.
- The product is companion-first. Serious Engineer and project-manager modes remain first-class
  conversational workflows grounded in real repos, tests, evidence, plans, and open loops; they do
  not turn TempestMiku into a generic agent marketplace.

## Current workspace state

- Rust workspace, edition 2024. Members are discovered by `crates/*` and `apps/*`.
- Existing crates/apps:
  - `crates/tm-core`: message types, streaming agent loop, result shaping, and the `LlmClient`,
    `Sandbox`, and `EventSink` traits.
  - `crates/tm-llm`: OpenAI-compatible streaming SSE client.
  - `crates/tm-artifacts`: session artifacts and content-addressed blob storage.
  - `crates/tm-host`: host registry, exact capability grants, approval policy, resource registry,
    linked folders, `fs.*`, `code.*`, and argv-vector `proc.run`.
  - `crates/tm-modes`: mode catalog, routing, voice caps, and configurable mode/skill asset status.
  - `crates/tm-agents`: actor lifecycle, mailbox, supervision, orchestration calls, and `agent://` /
    `history://` resources.
  - `crates/tm-memory`: deterministic extraction/redaction, reflections and summaries, durable scoped
    episodic/semantic records, local embeddings, deterministic hybrid RRF, proposals, jobs,
    tombstones, and readiness contracts.
  - `crates/tm-drive`: local-first drive entries, transducers, virtual directories/search, organizer
    proposals, linked-folder policy, and `drive://` resources.
  - `crates/tm-egress`: disabled-by-default exact-destination HTTPS, DNS/IP/redirect policy,
    cross-instance budgets, durable mutation intents, audit/revocation, and opaque
    destination-scoped secret handles.
  - `crates/tm-mcp`: selected MCP `2025-11-25` catalog/lifecycle, strict bounded schemas/results,
    Streamable HTTP JSON/SSE/resume, untrusted provenance envelopes, and durable approved mutations.
  - `crates/tm-worker-protocol`: signed coordinator/worker jobs, durable status contracts, remote
    host/resource registration, coordinator-owned approval resolution, and verified artifact
    localization.
  - `crates/tm-lang`: the sole `execute(code)` runtime: `tm` lexer, parser, checker, persistent
    interpreter, effect machine, host/resource/artifact wiring, and `Sandbox` adapter.
  - `crates/tm-server`: authenticated axum session API, durable turn queue, replayable SSE/event
    store, ordered Postgres migrations, supervised runtime roles, durable approvals/dreams/cron/drive
    state, project and memory authority, artifact routes, Serious Engineer backend path, and OMP ACP
    bridge.
  - `apps/tm-cli`: CLI wiring the LLM client, streaming agent loop, and sole tm-lang sandbox.
  - `apps/tm-worker`: minimal authenticated remote linked-host executor for the selected homolab
    deployment; it is not a second session/model server.
  - `apps/tm-e2e`: local/dev public-API workflow harness for scripted and opt-in live session smoke
    coverage.
  - `clients/miku_flutter`: chat-first Flutter/Web and Android client over the typed HTTP/SSE
    contracts. Conversation, history, projects, scoped Drive/resources, approvals, reviewed changes,
    notifications, imports, modes, and editable voice drafts are connected; §27.4.1 records exact
    platform coverage.
- `tm-server` is the sole production authority for sessions, turns, projects, memory, approvals,
  scheduling, and replay. Postgres is the durable deployment store; SSE is the live and replay source
  of truth.
- Android uses in-app QR pairing, secure token storage, HTTPS-only release networking, encrypted
  provider-neutral push, authenticated actions, share/selection/capture review, and explicit
  current/new-session sends. Voice capture is foreground-only and review-before-send. Local ASR is
  the default; an owner-selected fixed-destination home ASR service is optional, with no silent
  fallback in either direction.
- Production keeps the coordinator as the sole authority and may expose one signed `tm-worker` on a
  linked homolab host. The accepted isolation contract covers hostile workloads on a trusted
  owner-controlled host kernel; hostile-kernel containment and microVM isolation are not claimed.
- `tm-trace`, cloud sync/CRDT, AST/LSP, and additional sandbox backends remain demand-triggered for a
  concrete user or second consumer. Aggressive self-evolution is out of scope.

## Read before changing code

Start with the narrowest current-state document for the task:

- Product contract and navigation: `docs/PRODUCT.md`, then `docs/README.md`.
- Core runtime: `docs/design/core/01-tldr-the-bet.md`, `03-core-principles.md`,
  `04-architecture.md`, `05-agent-loop.md`, `06-repl-sandbox.md`, `07-host-sdk.md`,
  `08-security-model.md`, `09-context-artifacts.md`, and `10-rust-implementation.md`.
- Product layer: `docs/design/product/20-product-overview.md`, `21-persona-and-modes.md`,
  `22-memory-dreaming.md`, `23-agents-orchestration.md`, `24-drive-storage.md`,
  `25-coding-agent-sdk.md`, `26-self-evolution.md`, `27-server-and-clients.md`,
  `29-parity-baseline.md`, and `30-projects.md`.
- `tm` language: `docs/design/tm/README.md` and `docs/design/tm/07-language-reference.md`.
- Operation: `docs/running-miku.md` and `docs/deploy-coordinator-worker.md`.
- Historical acceptance records: `docs/history/README.md`. Consult these only when a change touches
  an already-proven contract; they are evidence, not an implementation queue.

No pre-committed roadmap remains. Scope new work from a concrete user need or second consumer and
preserve the current contracts unless the user explicitly changes them.

## Architectural invariants

- One model-visible tool: `execute(code)`. Capabilities are SDK namespaces the code calls, not chat-native tool sprawl.
- Streaming is the source of truth. `LlmClient::chat_stream` drives the loop; non-streaming chat is just drain + assemble.
- The agent loop owns message accumulation, tool-call execution, result shaping, and `EventSink` callbacks. Do not fork a second loop for product features.
- Sandbox backends sit behind `Sandbox` / `Session`. Product code must not depend on the current stub behavior.
- Capabilities are config and exact per-turn grants, not hardcoded privilege. Handler/link/drive
  registration never grants authority, previous turns never leak grants forward, and unknown
  capability/resource schemes fail closed.
- Real-repo reach is curated: `proc.run(cmd, args)` with argv-vector execution, allowlist, linked cwd, and approval. Never add a raw shell escape hatch.
- Artifacts and large outputs should spill to the artifact store. Do not push large tool/sub-agent output into model context.
- Product features extend the core; they do not weaken the core bet or bypass approval/security boundaries.

## Change boundaries

- Keep the native/OMP boundary boring: OMP ACP remains replaceable; tm-lang uses the existing
  `Sandbox` and host approval/artifact/resource boundaries.
- Do not record new real speech without explicit speaker consent or infer CER without an exact
  reference. Third-party cloud ASR, silent engine fallback, and automatic send remain out.
- Do not start `tm-trace`, cloud sync/CRDT, AST/LSP, or another sandbox backend without a concrete
  user or second consumer.
- Do not add aggressive autonomous identity/config/source/deployment rewrites, automatic persona
  activation, raw `SOUL.md` mutation, Firebase/FCM, raw shell/ambient authority, or chat-native tool
  sprawl.

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
- Strict Rust quality gate: `cargo fmt --all --check` followed by
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- Add narrow tests for the behavior you change before relying on the full suite.
- For streaming changes, use scripted streams; do not require network.
- Live OpenAI tests must stay opt-in/gated by environment and must not run in normal `cargo test`.
- For approval/security work, test the denial path, timeout/default-deny behavior, and fail-closed unknown capability/resource behavior.
- For sandbox work, test persistence across cells, reset, timeout/cancel, output caps, display/artifact capture, and blocked network/filesystem access.

## Commit messages

- Use the repository Conventional Commit dialect in `docs/commit-messages.md`: `<type>(<machine>/<scope>): <subject>`.
- Prefer stable machine/scope pairs from the spec, for example `core/agent-loop`, `sandbox/tm`, `host/approval`, `server/sse`, `web/approvals`, `docs/product`, or `repo/workspace`.
- Split unrelated intents into separate commits; keep tests, callsite migrations, and required docs with the behavior they prove or describe.

## Documentation rules

- Keep `docs/PRODUCT.md`, the relevant `docs/design/**` section, and implementation behavior aligned.
- Cross-references use section numbers (`§NN`) and should remain path-independent.
- `docs/history/ROADMAP.md` and `docs/evidence/**` are delivery provenance, not the active product
  definition. Do not rewrite retained reports; add new dated evidence when a changed contract needs
  a new acceptance record.
- Preserve accurate statements about the local command matrix and physical Android HTTPS
  pairing/chat/replay/revocation coverage when those contracts change.
