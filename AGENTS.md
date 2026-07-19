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
  - `crates/tm-artifacts`: session artifacts and content-addressed blob storage.
  - `crates/tm-host`: host registry, capability grants, approval policy, resource registry, linked folders, `fs.*`, `code.*`, and argv-vector `proc.run`.
  - `crates/tm-modes`: mode catalog, routing, voice caps, and configurable mode/skill asset status.
  - `crates/tm-agents`: actor lifecycle, mailbox, `agents.run/spawn/parallel/msg/send/broadcast/wait/inbox/list/cancel/pipeline`, supervision defaults, and `agent://` / `history://` resources.
  - `crates/tm-memory`: dream queue contracts, deterministic extraction/redaction, reflections,
    summaries, proposal types, P8.1 recall evaluation, durable P8.2 episodic/semantic record,
    job/tombstone/readiness contracts, and P8.3 local embedding plus deterministic hybrid-RRF
    contracts.
  - `crates/tm-drive`: local-first drive entries, transducers, virtual directories/search, organizer proposals, linked-folder policy, and `drive://` resources.
  - `crates/tm-egress`: disabled-by-default exact-destination HTTPS, DNS/IP/redirect policy,
    cross-instance budgets, durable mutation intents, audit/revocation, and opaque destination-scoped
    secret handles.
  - `crates/tm-mcp`: selected MCP `2025-11-25` catalog/lifecycle, strict bounded schemas/results,
    Streamable HTTP JSON/SSE/resume, untrusted provenance envelopes, and durable approved mutations.
  - `crates/tm-lang`: the sole `execute(code)` runtime: `tm` lexer, parser, checker, persistent
    interpreter, effect machine, host/resource/artifact wiring, and `Sandbox` adapter. Its T0-T7
    public HTTP/SSE, approval, runtime-loss, real-Postgres replay, client, and historical comparative
    fluency gates pass.
  - `crates/tm-server`: authenticated axum session API, durable turn queue, replayable SSE/event store, ordered Postgres migrations including the P8.3 scoped hybrid-memory spine, supervised runtime roles, durable approvals/dreams/cron/drive state, artifact routes, Serious Engineer backend path, and OMP ACP bridge.
  - `apps/tm-cli`: CLI wiring the LLM client, streaming agent loop, and sole tm-lang sandbox.
  - `apps/tm-e2e`: local/dev public-API workflow harness for scripted and opt-in live session smoke
    coverage. The retired TypeScript-versus-tm runner survives only as historical evidence/corpus.
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
  current/new-session exact-once, duplicate-intent, and process-death proof. The whole-roadmap pass
  resumed P6.6 on 2026-07-18 and implemented foreground-only bounded PCM capture, explicit verified
  model install, local sherpa-onnx inference, on-device Traditional-Chinese conversion, cleanup,
  editable review, and explicit current/new-session send with no auto-send. Local remains default;
  an optional fixed-destination home-hosted ASR engine is an explicit owner choice and neither engine
  may silently fall back to the other. Its software,
  host-inference, signed-package, exact model activation, and airplane-mode cold-start inspection
  gates pass. A later consented recording after the immutable-buffer fix reached editable review
  without sending but failed quality; that historical checkpoint was superseded by the completed
  signed closeout. The latest `1.0.3+4` diagnostics/self-hosted package installed
  in place, matched the host hash, retained pairing, and exposed the configured production TEA-ASR
  label/model behind explicit disclosure and selection. The same-device streaming/offline-candidate
  synthetic A/B passes all frozen quality/resource/thermal gates. Permission denial, model delete/
  reinstall, interrupted/corrupt recovery, restart persistence, and backup/external-residue gates
  also pass. The production background/process-death cleanup, healthy and stopped-upstream
  self-hosted paths, and distinct exact-once current/new sends pass. A consented production-local
  10-item corpus passes mean CER `0.132922`, p90 `0.241379`, and code-switch mean `0.281980`, with no
  empty, truncated, or signal-warning item. P6.6 and full P6 are closed. See
  `docs/evidence/2026-07-18-p6-6-on-device-asr-resumption.md` and
  `docs/evidence/2026-07-19-p6-6-real-speaker-eval.json`. P8 is closed: the frozen lexical
  control, durable scoped records, local embeddings, hybrid turn integration, exact bounded
  inspection, typed approved extraction, and lumo quality/restart/fallback gates all pass. Its
  post-closeout contract also requires serialized scope revocation/approved writes, monotonic
  scope-revision-pinned embedding activation, authority-first dense queries, and balanced 2/2/1
  prompt allocation. The owner-priority tm-lang runtime slice is closed and default. The real
  `omp/16.4.8` P0a dogfood and retained `gpt-5.5` P2 General/grounding/Serious live voice rubric pass.
  M2 `tools.call`, P7.2b approval-backed persona addenda, P9 egress/opaque secrets, and P10 selected
  MCP/live research are closed. M4 is closed for the owner-selected homolab production contract:
  both Linux profiles pass their disposable canaries, and the persistent native x86_64 NixOS
  service proves exact systemd delegation, cgroup exclusivity, representative workload headroom,
  retained identity-bound reporting, switch persistence, and restart stability. Its threat model is
  a hostile workload on the trusted owner-controlled host kernel; hostile-kernel containment and
  microVM isolation are explicitly unclaimed. The earlier lumo preflight remains a useful
  fail-closed result because that host lacks the `memory` controller. `tm-trace`,
  cloud sync, AST/LSP, and any future second
  sandbox backend are demand-triggered; aggressive self-evolution is out.

## Read before changing code

Start with the narrowest docs for the task:

- Overview: `docs/README.md`, `docs/design/README.md`.
- Core architecture: `docs/design/core/01-tldr-the-bet.md`, `03-core-principles.md`, `04-architecture.md`, `05-agent-loop.md`, `06-repl-sandbox.md`, `07-host-sdk.md`, `08-security-model.md`, `09-context-artifacts.md`, `10-rust-implementation.md`; core roadmap lives in `ROADMAP.md` (§14).
- Product layer: `docs/design/product/20-product-overview.md`, `21-persona-and-modes.md`, `22-memory-dreaming.md`, `23-agents-orchestration.md`, `24-drive-storage.md`, `25-coding-agent-sdk.md`, `26-self-evolution.md`, `27-server-and-clients.md`, `29-parity-baseline.md`; product roadmap lives in `ROADMAP.md` (§28).
- P0a live OMP evidence: `docs/evidence/2026-07-18-p0a-omp-acp-live.json`.
- P2 dialectic/live voice evidence: `docs/evidence/2026-07-18-p2-voice-dialectic.md` and
  `docs/evidence/2026-07-18-p2-voice-live.json`.
- P6.6 resumption evidence: `docs/evidence/2026-07-18-p6-6-on-device-asr-resumption.md`.
- P6.6 consented real-speaker aggregate: `docs/evidence/2026-07-19-p6-6-real-speaker-eval.json`.
- P7.2b closeout evidence: `docs/evidence/2026-07-18-p7-2b-persona-addenda.md`.
- P9 closeout evidence: `docs/evidence/2026-07-18-p9-egress-secret-broker.md`.
- P10 closeout evidence: `docs/evidence/2026-07-18-p10-mcp-runtime.md`.
- M4 partial Linux isolation evidence: `docs/evidence/2026-07-18-m4-linux-proc-isolation.md`.
- M4 hardened Linux evidence: `docs/evidence/2026-07-18-m4-linux-hardened-v1.md`.
- M4 homolab production evidence: `docs/evidence/2026-07-19-m4-production-homolab.md`.
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

1. No committed roadmap milestone remains open. Start demand-triggered work only for a concrete user
   or second consumer, preserving the closed acceptance boundaries.
2. Keep the native/OMP boundary boring: OMP ACP remains replaceable; tm-lang uses the existing
   `Sandbox` and host approval/artifact/resource boundaries.

Do not start yet unless explicitly requested:

- Do not record new real speech without explicit speaker consent or infer CER without an exact
  reference; preserve the P6.6 physical claim only with its signed Android matrix. Third-party cloud ASR,
  silent engine fallback, and automatic send remain out.
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
- Prefer stable machine/scope pairs from the spec, for example `core/agent-loop`, `sandbox/tm`, `host/approval`, `server/sse`, `web/approvals`, `docs/roadmap`, or `repo/workspace`.
- Split unrelated intents into separate commits; keep tests, callsite migrations, and required docs with the behavior they prove or describe.

## Documentation rules

- Keep design docs and implementation aligned. If behavior changes milestone scope, update the relevant `docs/design/**` section in the same PR/change.
- Cross-references use section numbers (`§NN`) and should remain path-independent.
- Do not mark a roadmap stage done until its acceptance checks pass.
- Keep the closed audit evidence accurate: the local command matrix and physical Android HTTPS
  pairing/chat/replay/revocation canary must remain represented when contracts or gates change.
