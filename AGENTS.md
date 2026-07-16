# TempestMiku agent guide

## Project identity

- TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: self-hosted, single-user, characterful personal AI companion plus code-execution agent runtime.
- Parity comes first. Shipped P0-P3 slices must keep preserving current behavior: Miku voice, mode router, memory recall/profile, and manual approvals.
- This is not a generic coding agent. Product work must preserve the constant Miku identity and the serious-mode voice cap.
- The long-range product direction is companion-first. Serious Engineer and project-manager modes
  remain first-class conversational workflows grounded in real repos, tests, evidence, plans, and
  open loops; they do not turn TempestMiku into a generic agent marketplace.

## Current workspace state

- Rust workspace, edition 2024. Members are discovered by `crates/*` and `apps/*`.
- Existing crates/apps:
  - `crates/tm-core`: message types, streaming agent loop, result shaping, `LlmClient`, `Sandbox`, `EventSink` traits.
  - `crates/tm-llm`: OpenAI-compatible streaming SSE client.
  - `crates/tm-sandbox`: M0 `StubSandbox` plus the M1 `deno_core` JS/TS backend with persistent cells, timeout/reset, output spill, resource reads, SDK prelude, and host-call bridge.
  - `crates/tm-artifacts`: session artifacts and content-addressed blob storage.
  - `crates/tm-host`: host registry, capability grants, approval policy, resource registry, linked folders, `fs.*`, `code.*`, and argv-vector `proc.run`.
  - `crates/tm-modes`: mode catalog, routing, voice caps, and configurable mode/skill asset status.
  - `crates/tm-agents`: actor lifecycle, mailbox, `agents.run/spawn/parallel/msg/send/broadcast/wait/inbox/list/cancel/pipeline`, supervision defaults, and `agent://` / `history://` resources.
  - `crates/tm-memory`: dream queue contracts, deterministic extraction/redaction, reflections,
    summaries, proposal types, P8.1 recall evaluation, durable P8.2 episodic/semantic record,
    job/tombstone/readiness contracts, and P8.3 local embedding plus deterministic hybrid-RRF
    contracts.
  - `crates/tm-drive`: local-first drive entries, transducers, virtual directories/search, organizer proposals, linked-folder policy, and `drive://` resources.
  - `crates/tm-server`: authenticated axum session API, durable turn queue, replayable SSE/event store, ordered Postgres migrations including the P8.3 scoped hybrid-memory spine, supervised runtime roles, durable approvals/dreams/cron/drive state, artifact routes, Serious Engineer backend path, and OMP ACP bridge.
  - `apps/tm-cli`: CLI wiring the LLM client, streaming agent loop, and Deno sandbox by default; `--stub-sandbox` remains for protocol tests.
  - `apps/tm-e2e`: local/dev public-API workflow harness for scripted and opt-in live session smoke coverage.
  - `clients/miku_flutter` / `clients/miku_web`: client scaffolds and smoke coverage.
- The P4/P5 production-hardening implementation has landed: ordered checksummed migrations,
  transactional session end, durable approval effects, fenced dream/cron leases, `api|worker|all`
  supervision, Postgres drive metadata/link tombstones, server-owned memory authority, durable turns,
  and gap-free `session_event` replay. The local automated matrix, including gated/two-process
  Postgres recovery, signed APK builds, certificate verification, and authenticated Playwright,
  passes. A physical Android release canary over Tailscale Serve HTTPS also proves in-app QR pairing,
  durable chat, cold-start recovery, exact-once offline replay, and immediate credential revocation.
  The audit hardening gate is closed for the documented single-owner deployment.
- Android now has in-app QR pairing, secure token storage, HTTPS-only release networking, backup
  exclusion, release-signing enforcement, encrypted provider-neutral push registration/outbox, and
  authenticated notification actions. Production UnifiedPush/ntfy delivery and the remote
  killed-process canary are closed. The P6.2 Android text/share-target implementation and physical
  current/new-session canary are closed. P6.3 adds a bounded selected-text action through the same
  review path; its local gates and signed Android 15 cancel/current/new-session exact-once canary are
  closed. P6.4 actionable notifications are closed with local Kotlin/Flutter/server/Postgres proof
  plus signed Android 15 foreground/background/killed-process, exact-once reply/retry, terminal
  failure, and cold-start evidence. P6.5 quick capture is closed with deterministic tests, signed
  APK inspection, and real launcher-shortcut/Quick Settings foreground/background/killed-process,
  current/new-session exact-once, duplicate-intent, and process-death proof. P6.6 on-device voice
  capture/closeout was explicitly deferred on 2026-07-15 after its interface spike and isolated
  Android feasibility/Taiwan-Mandarin benchmark. P6.1-P6.5 remain closed; P6.6 and full P6 closeout
  remain incomplete. Preserve the harness and evidence, but do not add production microphone,
  model-install/runtime, fallback, or send work unless the owner explicitly resumes the slice. The
  independent evidence and resume contract live at
  `docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md`. P8 is closed: the frozen lexical
  control, durable scoped records, local embeddings, hybrid turn integration, exact bounded
  inspection, typed approved extraction, and lumo quality/restart/fallback gates all pass. Its
  post-closeout contract also requires serialized scope revocation/approved writes, monotonic
  scope-revision-pinned embedding activation, authority-first dense queries, and balanced 2/2/1
  prompt allocation. P7.2b
  approval-backed persona addenda is now active, followed by P9 egress/secrets and P10 MCP/live
  research. `tm-trace`, cloud
  sync, AST/LSP, and extra sandbox backends are demand-triggered; aggressive self-evolution is out.

## Read before changing code

Start with the narrowest docs for the task:

- Overview: `docs/README.md`, `docs/design/README.md`.
- Core architecture: `docs/design/core/01-tldr-the-bet.md`, `03-core-principles.md`, `04-architecture.md`, `05-agent-loop.md`, `06-repl-sandbox.md`, `07-host-sdk.md`, `08-security-model.md`, `09-context-artifacts.md`, `10-rust-implementation.md`; core roadmap lives in `ROADMAP.md` (§14).
- Product layer: `docs/design/product/20-product-overview.md`, `21-persona-and-modes.md`, `22-memory-dreaming.md`, `23-agents-orchestration.md`, `24-drive-storage.md`, `25-coding-agent-sdk.md`, `26-self-evolution.md`, `27-server-and-clients.md`, `29-parity-baseline.md`; product roadmap lives in `ROADMAP.md` (§28).
- Deferred P6.6 evidence: `docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md`.
- P8.1 recall baseline evidence: `docs/evidence/2026-07-15-p8-1-recall-baseline.md`.
- P8.2 durable-memory evidence: `docs/evidence/2026-07-15-p8-2-durable-memory-spine.md`.
- P8.3 local-embedding/hybrid evidence: `docs/evidence/2026-07-15-p8-3-local-embeddings-hybrid.md`.
- P8.4/P8.5 closeout evidence: `docs/evidence/2026-07-15-p8-5-fuller-memory.md`.
- Roadmap execution order lives in `ROADMAP.md` under “Execution plan from current workspace”. Follow it unless the user explicitly changes priority.

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

## Milestone sequencing

Default next work:

1. Implement P7.2b from `TODO.md`: Auto mode may create bounded, evidence-backed persona addendum
   proposals, but durable manual approval is the only activation/rollback path. Preserve identity,
   `SOUL.md`, safety, modes, voice caps, skills, scopes, and capability invariants.
2. Continue with P9 egress/opaque secrets only after P7.2b closes, then P10 MCP/live research.
3. Keep the native/OMP boundary boring: OMP ACP remains replaceable, while native Deno remains the
   dogfood path for `fs.*` / `code.*` / `proc.*`, artifacts, and HTTP-routed approvals.

Do not start yet unless explicitly requested:

- P6.6 product work unless the owner explicitly resumes it after reviewing the independent evidence
  note. Incidental harness build hygiene is allowed; microphone permission, production weights,
  model installer/runtime, server/cloud fallback, and automatic send are not.
- MCP or live external research before P9 egress, redirect, budget, audit, revocation, and opaque
  secret gates.
- `tm-trace`, cloud sync/CRDT, AST/LSP, or extra sandbox backends without a concrete second consumer.
- Aggressive autonomous identity/config/source/deployment rewrites, automatic persona activation,
  raw `SOUL.md` mutation, Firebase/FCM, raw shell/ambient authority, or chat-native tool sprawl.

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
- Prefer stable machine/scope pairs from the spec, for example `core/agent-loop`, `sandbox/deno`, `host/approval`, `server/sse`, `web/approvals`, `docs/roadmap`, or `repo/workspace`.
- Split unrelated intents into separate commits; keep tests, callsite migrations, and required docs with the behavior they prove or describe.

## Documentation rules

- Keep design docs and implementation aligned. If behavior changes milestone scope, update the relevant `docs/design/**` section in the same PR/change.
- Cross-references use section numbers (`§NN`) and should remain path-independent.
- Do not mark a roadmap stage done until its acceptance checks pass.
- Keep the closed audit evidence accurate: the local command matrix and physical Android HTTPS
  pairing/chat/replay/revocation canary must remain represented when contracts or gates change.
