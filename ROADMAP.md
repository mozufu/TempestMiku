# TempestMiku roadmap

This is the canonical roadmap for TempestMiku. Design-doc section references remain stable:

- Core runtime roadmap: §14.
- Product roadmap and execution order: §28.

TempestMiku is a **rewrite** (§29): reach behavioral parity with the current `hermes-agent`, then deepen.
Dogfood in this order: **coding agent → project manager → personal assistant**. The first useful slice must help build TempestMiku itself, then manage the project around that work, then broaden into the full companion.

## Core runtime milestones (§14)

| Milestone | Deliverable |
|---|---|
| **M0 — streaming protocol skeleton** | Streaming-first OpenAI client (SSE) + the agent loop consuming deltas live + an `EventSink` (assistant tokens token-by-token) + a stub sandbox (echoes code). Validates the streaming message protocol end-to-end; tests drive a scripted stream with no network. |
| **M1 — real REPL** | `deno_core` backend; ops for `display`, `http.get` (allowlisted), `artifacts`; result-shaping + artifact spill; system prompt. |
| **M2 — disclosure + tools** | `tools.search/docs/call`; `tm-mcp` imports external MCP tools into the catalog; secrets-by-reference. |
| **M3 — production loop** | Approval gates, audit log, session pooling, deterministic replay. (Streaming landed in M0.) |
| **M4 — backends & hardening** | `rquickjs` + Python-subprocess backends; Linux isolation (seccomp/cgroups/microVM); egress hardening. |

## Product roadmap (§28)

| Stage | Deliverable | Modes (§21) | Needs (core) | Acceptance (incl. parity §29.5) |
|---|---|---|---|---|
| **P0a — OMP ACP bridge** | Transitional Serious Engineer backend: `tm-server` delegates coding execution to a pinned `omp acp` process, maps ACP progress/diffs/finals into TempestMiku SSE + event log, and routes ACP permission requests through TempestMiku approvals while Miku owns persona/final voice | 4/5 | M0 + `tm-server` | real TempestMiku patch edit + targeted test succeeds through OMP ACP; progress replays via `Last-Event-ID`; at least one permission path is resolved by TempestMiku approval UI/API; output is mirrored to `artifact://`/resource refs with provenance; serious voice cap is `off`; OMP remains a replaceable backend, not the final SDK surface |
| **P0 — coding-agent dogfood** | Serious Engineer slice for TempestMiku itself: `fs.*`/`code.*`/`proc.*` (§25), linked repo, artifacts, minimal project memory, streaming CLI | 4 | M0–M2 | real linked-repo patch edit + targeted test succeeds; output spills to `artifact://`; project summary/open-loop memory is recalled next session; approvals gate destructive/out-of-grant ops; serious voice cap is `off` |
| **P1 — project manager + remote control** | Chief-of-staff layer over the coding loop: mode router, project plans/open loops/decisions, session event log, SSE/API project surface, project promotion, and Flutter Web/PWA remote-control client | 1/2/4 | M1–M2 | router fires; user can lock; badge + voice cap work; project status survives reconnect; Miku can turn a coding session into next actions without losing provenance; a phone/browser can attach to the same session stream, send a message, resolve an approval, open resource links, promote session workspace/artifacts into `project://`, and resume via `Last-Event-ID` |
| **P2 — personal assistant** | Full companion baseline: Miku voice, profile/user memory, personal-assistant state capture, negative-state grounding, bounded proactivity | 1–3 | M1–M2 | replies in Miku voice when appropriate; memory context + user profile load; personal reminders/open loops are captured with approval; grounding mode preserves the health-over-productivity rule |
| **P3 — handoff + orchestration** ✓ | P3 MVP `agents.run/spawn/parallel/msg` (§23): actor sessions + message passing + supervision; Handoff mode + brief template; actor lifecycle events in SSE stream | 5 | M2 | ✓ fan-out N workers, only digest returns to parent context; ✓ siblings coordinate via one-shot msg; ✓ crashing sub-agent isolated + `actor_failed` in SSE; ✓ handoff matches `oh-my-pi-handoff` |
| **P3+ — live actor mailbox** ✓ | Live MPSC resident delivery (`agents.send/wait/inbox/list/broadcast/cancel/pipeline`); active supervision (restart/subtree cancel); child approval routing through `ApprovalBroker`; DAG acyclicity + protocol invariants; Flutter smoke coverage | 5 | M2 | live siblings coordinate via `send`/`wait`; child cancel yields replayable `Cancelled` event; child approval requests reach the user; restart/timeout/subtree-failure decisions are replayable; Flutter attach/approve/reconnect pass |
| **P4 — dreaming + proactivity** ✓ | consolidation + `skills/` generation (§22/§26); scheduler/cron (§27.2) | all | — | transactional session end, durable approvals/effects, fenced leases, enforced cron bounds, supervised roles, recovery tests, and final client verification pass |
| **P5 — drive + research** ✓ | `drive.*` + auto-organizer (§24): transducers + virtual dirs; deep-research workspace (P3+P4+P5) | 4/5 | — | Postgres metadata/organizer/link persistence, CAS application, tombstones, startup link revalidation, and final restart/client verification pass |
| **P6 — Android package + OS integrations (UnifiedPush canary pending)** | Ship the Android target of the existing Flutter client after the hardened server contracts pass | all | — | Android/Web share authenticated durable turns and SSE; secure pairing/release gates and the physical-device canary pass; encrypted provider-neutral registration, leased push outbox, private approval actions, a production UnifiedPush/ntfy path, and deterministic provider probes landed; killed-process remote delivery and broader OS integrations remain |
| **P7 — self-evolution tiers + hardening** | tiers (§26); isolation / egress hardening | all | M3–M4 | tier switch changes write-scope; conservative writes only memory+skills; audit trail |

**Parity gate (§29.5):** P0–P4 are not "done" until they reproduce the current behavior for their
slice (coding reach, project continuity, voice, mode router, memory recall, approvals). New capability layers on *after* parity.

## Execution plan from current workspace (2026-07-10)

Current implementation has advanced past the original M0-only substrate. The workspace now includes
`tm-core`, `tm-llm`, `tm-sandbox` with both `StubSandbox` and a `deno_core` backend, `tm-artifacts`,
`tm-host`, `tm-modes`, `tm-agents`, `tm-server`, `apps/tm-cli`, and client scaffolds under
`clients/`. The implemented path covers M0, M1, host/approval/resource foundations, P0a OMP ACP
bridging, the native P0 Serious Engineer dogfood slice, the CLI native cutover proof for
linked-repo edit/run/artifact workflows, native Deno HTTP approvals for the Serious Engineer backend,
the P1 project-manager remote-control surface, the P2 personal-assistant baseline, the full P3
actor orchestration surface, and the P4 dreaming/scheduler slice. P3 ships `agents.run/spawn/parallel/msg`, the `agent://` + `history://`
resource handlers, child artifact spill + transcript history, actor lifecycle events in the SSE
stream, Handoff mode wired from the mode catalog, and `CapabilityGrants` glob permit support. P3+
is also closed: bounded per-actor inbox queues, `Agent::run` inbox draining,
`agents.send/broadcast/wait/inbox/list/cancel/pipeline`, delivered-message and status lifecycle
events, actor-id threading through `InvocationCtx`, child approvals routed through the live
`HttpApprovalPolicy` / `ApprovalBroker`, parent-session child artifacts, parent-driven actor cancel
tokens with replayable `actor_cancelled` events, active restart supervision (`one_for_one`,
`one_for_all`, `rest_for_one`), wall-clock actor timeouts, fail-fast sibling-subtree supervision,
typed parent `SessionEvent` resource links, `actor_resources_linked` provenance events, and
tm-e2e/Flutter actor smoke coverage. P4's test-backed mechanisms include dream queue records,
deterministic dream extraction,
redaction/config guardrails, session/reflection/rollup summaries, approval-gated memory and skill
proposals, Postgres FTS recall coverage, cron job/run helpers, and Flutter/Web smoke coverage for
dream-origin proposals and replay. P5's test-backed mechanisms include `tm-drive`, `drive.*`,
local-first
blob-backed entries, transducers, search/virtual-dir feeds, approval-gated dropped-file writes,
shared `write_proposal` organizer events, `drive://` resource resolution, replayable drive mutation
events, project links that mint `FsPolicy` and memory scopes, and local research fan-out over drive
docs with per-child budgets are covered by deterministic Rust/e2e/client tests plus gated Postgres
schema/resource/replay tests.
The complete local command matrix passes, including clean-schema Postgres, split API/worker restart,
authenticated Playwright, Flutter, signed arm64 Android APKs, certificate, RustSec, and secret scans.
A physical Android 15 release canary over Tailscale Serve HTTPS also proves in-app QR confirmation,
durable chat, cold-start credential/session recovery, exact-once replay of a turn completed while the
app was stopped, and immediate SSE/API loss after device revocation. Managed sandboxes may need
elevated permissions for tests that bind `127.0.0.1`.

The audit hardening implementation is now wired. Postgres upgrades through ordered checksummed
`schema_migrations`; session end and dream enqueue are transactional; device credentials, turns,
approval requests/effects, dream/cron leases, and drive metadata/link tombstones are durable. Runtime
roles are explicit: `api` serves HTTP, `worker` dispatches turns/effects/dreams/cron, and `all` does
both. Worker roles require Postgres and drain under a supervised shutdown grace. The public API uses
server-owned owner/scope authority, `202` idempotent turns, and one durable `session_event` SSE
envelope. Android/Web use the same device records through bearer tokens or an HttpOnly cookie.

The audit hardening gate is closed for the documented single-owner deployment. Production exposure
remains loopback-only behind an HTTPS reverse proxy or Tailscale Serve, and Postgres remains
mandatory outside loopback and for embedded workers. This gate does not broaden the deployment
target into multi-tenancy or arbitrary public-internet hosting.

Also not production-complete: `tm-mcp`, `tm-trace`, fuller `tm-memory` mechanisms beyond the P4
acceptance slice (pgvector dense retrieval, graph extraction, LLM-backed extraction, richer scoped
`memory.*` APIs), first-class skill import/reload/write surfaces, a production Android push provider and remaining OS integration, optional
drive cloud sync, generated SDK docs from the runtime registry, and production egress/secret
hardening.

Sizing below is **solo focused engineering days**. Treat it as sequencing pressure, not a promise:
each milestone is done only when its acceptance checks pass.

| Order | Milestone | Estimate | Tasks | Acceptance |
|---:|---|---:|---|---|
| 0 | **DONE — M0 closeout: streaming skeleton locked** | 0.5–1d | Freeze stream event contract; keep OpenAI SSE adapter tests; keep CLI token streaming; document current stub-sandbox boundary. | `cargo test`; scripted stream test proves token deltas, tool-call assembly, stub eval, and final answer path. |
| 1 | **DONE — M1 real REPL + SDK spine** | 4–7d | Add `deno_core` backend behind `Sandbox`; implement persistent cells, cancel/reset, `display`, `http.get` allowlist, artifact writes; add `tm-artifacts` spill store; enforce `CellBudget` and output caps. | Rust tests cover TypeScript cells, persistent state, reset, display/print capture, blocked raw APIs/network, timeout/cancel, shaped output truncation, and artifact spill/readback. |
| 2 | **DONE — host registry + approvals foundation** | 3–5d | Add `tm-host`; define `HostFn`, capability grants, `ResourceRegistry`, `ApprovalPolicy`; wire SDK dispatch from sandbox ops; fail closed on unknown schemes/capabilities. | Unit tests prove denied capability cannot execute, approval timeout denies by default, `artifact://` resolves through the registry, linked paths cannot escape roots, and unsafe operations gate or fail closed. |
| 2.5 | **DONE — P0a OMP ACP bridge path** | 2–4d | Add an OMP ACP coding backend behind `tm-server`: spawn/connect a pinned `omp acp` process with generated linked-repo config, translate ACP session/progress/diff/final/permission messages into `session_events` + SSE, mirror outputs into artifacts/resource refs, and keep final user-facing response under Miku persona. | Tests cover ACP event normalization, diff/artifact events, generated approval mode, permission request round trips, approval route resolution, and replay from `Last-Event-ID`. Live `omp acp` dogfood remains backend-environment dependent. |
| 3 | **DONE — P0 coding-agent dogfood first pass** | 5–8d | Add config-declared linked-folder `FsPolicy`; implement `fs.*`, search, patch-only `code.edit`, `proc.run(cmd,args)` allowlist, artifact spill, minimal project memory, and a streaming CLI surface for Serious Engineer mode. | Tests prove linked-folder read/write/list/find, patch edit and stale-tag rejection, argv-vector `proc.run`, shell-string rejection, non-allowlisted denial, output spill, CLI Deno host ops, native Deno approve/deny/timeout through the HTTP approval route, serious-mode voice cap, and project summary/open-loop recall. |
| 4 | **DONE — P1 project manager + remote control** | 4–7d | Added the project-manager layer over the existing server: mode router lock/override, `ModeChanged` events, project/open-loop/decision views, session-to-project promotion, fuller session-scoped resource gateway, and Flutter Web/PWA remote-control polish. | Tests and smoke coverage prove reconnect replay from event id, router lock/unlock, project status and decisions across sessions, `POST /sessions/:id/promote` to `project://` resources with provenance, approval/resource workflows, and Flutter Web/PWA attach/send/watch/resume behavior. |
| 5 | **DONE — P2 personal assistant baseline** | 5–8d | Added the companion baseline: full persona overlay via SOUL + mode skill bundles, configurable Hermes asset path with degraded boot warning, profile/user recall, personal-assistant state capture, negative-state grounding, and bounded proactive reminder/open-loop capture through approval-backed memory writes. | Tests cover token/text streaming over SSE, active mode profiles loading SOUL + dedicated skills, profile/recall context injection, characterful non-serious voice caps, approval-gated profile/recall/reminder writes, `memory://` resources, negative-state no-write guardrails, and health-over-productivity/proactivity prompt bounds. |
| 6 | **DONE — P3: handoff + sub-agent actors** | 6–10d | Added `tm-agents` with actor lifecycle, mailbox, roster, `agents.run/spawn/parallel/msg`, `agent://` + `history://` resources, child artifact spill + transcript history, actor lifecycle events in SSE stream, supervision defaults, Handoff mode wired from catalog, `CapabilityGrants` glob permit. | ✓ Fan-out N workers, only digest returns to parent context; ✓ siblings coordinate via one-shot msg; ✓ child crash isolated + `actor_failed` in SSE log; ✓ handoff matches `oh-my-pi-handoff`. |
| 6+ | **DONE — P3+ live actor mailbox + supervision** | 4–8d | Landed per-actor bounded inbox + `Agent::run` inbox draining, `agents.send/broadcast/wait/inbox/list/cancel/pipeline`, child approval routing through the live broker, parent-session child artifacts, parent-driven cancel tokens with replayable `actor_cancelled`, plain-prose enforcement, live-wait DAG acyclicity checks, pipeline digest-reference wiring, active restart strategies (`one_for_one`, `one_for_all`, `rest_for_one`), wall-clock budget enforcement, fail-fast sibling-subtree supervision, status lifecycle events, typed parent `SessionEvent` actor/artifact/history links, and tm-e2e/Flutter actor smoke coverage. | Tests prove live sibling coordination, cancel/replay, child approval routing, restart decisions, actor timeouts, fail-fast sibling-subtree cancellation, event-log resource provenance, and Flutter attach/approve/reconnect smoke coverage. |
| 7 | **DONE — P4 dreaming + scheduler production hardening** | 6–10d | Added transactional lifecycle writes, durable approval requests/effects, fenced owner/epoch leases with heartbeat/retry limits, atomic cron cursor materialization, deny-only execution bounds, supervised worker roles, and recovery metrics. | Deterministic, gated, split-process, and physical-client verification cover stale-owner rejection, bounded retries, transactional enqueue, restart recovery, and authenticated replay. |
| 8 | **DONE — P5 drive + research workspace production hardening** | 5–9d | Added `DriveMetadataStore`, Postgres entries/proposals/organizer/corrections/version/link/tombstone state, CAS moves/apply, startup canonical-root revalidation, and the documented no-database historical metadata exception. | Deterministic, gated, split-process, and physical-client verification cover persistence, revocation, hydration, concurrent application, and restart-safe access. |
| 9 | **P6 UNIFIEDPUSH IMPLEMENTED — live canary pending** | 4–7d | The provider-neutral foundation now has a production UnifiedPush adapter with exact-origin/redirect policy and RFC 8291 payload encryption. Android uses the official connector, keeps notification handling native while killed, and registers its endpoint through device auth. Self-hosted ntfy plus the combined API/worker role are live on lumo behind Traefik; Firebase remains absent. | Deterministic encrypted-provider tests, Flutter tests, an arm64 APK build, and the live lumo health/migration/restart gate pass. Configure the Android distributor and prove request and resolution delivery while the signed release app is killed before marking P6 complete. |
| 10 | **DONE — P7.0 safety foundation + P7.1 managed skills** | 6–12d | P7.0 landed typed tiers/targets, least-authority effects, audit history, bounded replay, and Moderate review-only addenda. P7.1 adds approval-backed immutable managed skill versions, cross-process locked atomic activation/rollback, trigger-aware reload, and capability-gated `skill://` reads without allowing bundled/hand-authored replacement. Persona/mode apply, aggressive writes, MCP, and egress remain disabled. | Strict Rust/workspace, gated Postgres, Flutter/Web, and drift gates pass. Focused tests plus final `evolution-policy` evidence prove install, second-version activation, post-session approval recovery, rollback, reload, collision/tamper denial, and public resource reads. |
| 11 | **DONE — P7.2a managed mode addenda** | 2–4d | Moderate mode proposals install immutable description/routing-guidance versions behind manual approval, atomically activate/rollback under a cross-process lock, and compose on the next prompt without mutating `SOUL.md`, `modes.json`, capabilities, voice caps, scopes, skills, or route triggers. Persona apply remains disabled. | Focused crate/server tests and `tm-e2e record evolution-policy` prove apply, deny/timeout, stale-base denial, replay, next-turn composition, capability invariance, and rollback to the base catalog. |

### Immediate next task queue

1. **Active — close the P6 UnifiedPush canary** — lumo now runs the self-hosted ntfy service and
   combined API/worker role with one persistent encryption key and the exact endpoint origin.
   Configure the Android ntfy distributor, register the signed release, then capture approval
   request and resolution delivery while the app process is killed. Keep Firebase/FCM absent.
2. **Keep the native/OMP coding backend boundary boring** — OMP ACP remains replaceable, while the
   native Deno Serious Engineer backend is the dogfood path for `fs.*` / `code.*` / `proc.*`,
   artifacts, and HTTP-routed manual approvals. The network-free
   `tm-e2e record native-coding` gate now proves linked-repo edit/test/spill behavior,
   approve/deny/timeout effects, and durable turn-aware SSE replay through the public API.
3. **Keep P7.2a closed** — managed skills and guidance-only mode addenda have passed immutable
   installation, atomic activation/rollback, reload/composition, replay, and authority-invariance
   gates. Keep persona apply, direct mode-file/capability changes, aggressive writes, `tm-mcp`,
   `tm-trace`, live egress, and optional cloud sync deferred until a new roadmap slice is explicitly
   selected.

### Deferred SDK namespace placement

These are roadmap-owned deferred tasks, not loose TODOs:

| Namespace / surface | Target milestone | Placement note |
|---|---|---|
| `memory.*` | **P2 shipped; P4 production-complete** | P2 exposes minimum profile/user recall and state capture. P4 owns transactional dream enqueue, supervised fenced workers, durable proposal effects, summaries, Postgres FTS, and skill/memory proposals; pgvector, graph recall, LLM-backed extraction, and richer scoped APIs remain later expansion. |
| `agents.*` | **P3 ✓ / P3+ ✓** | P3 MVP shipped: `agents.run/spawn/parallel/msg`, actor lifecycle, mailbox/roster, `agent://`+`history://` resources, SSE lifecycle events, Handoff mode. P3+ shipped bounded live inbox delivery plus `send`, `broadcast`, `wait`, `inbox`, `list`, `cancel`, and `pipeline` with digest-reference stage wiring, child approval routing, plain-prose enforcement, live-wait DAG acyclicity checks, active restart supervision, sibling-subtree cancellation policy, wall-clock budgets, status lifecycle events, typed parent `SessionEvent` actor/artifact/history links, and actor smoke coverage. |
| `skills.*` / `skill://` reads | **P4 / P7.1 shipped; later import work deferred** | P4 emits approval-gated skill proposals from dreaming. P7.1 ships immutable managed versions, atomic activation/rollback, trigger-aware reload, provenance/audit, and capability-gated `skill://` read/list/preview. `skills.*` and MCP import/reload gates remain deferred. |
| `drive.*` | **P5 production-complete** | `tm-drive`, virtual dirs/search, transducers, approval-gated writes, `drive://`, project scopes, research fan-out, durable Postgres organizer/link/tombstone state, CAS, startup hydration, and final restart/client verification pass. |
| `http.*` hardening | **P7** | P5 research stays local-first over `drive://` docs. Live external research remains deferred until byte/request caps, redirect policy, audit logging, production allowlists, and secret handling are proven. |
| `secrets.use` | **P7** | Requires an opaque-handle secret broker, egress-scoped grants, and audit guarantees that never materialize secret values in JS heap, artifacts, or model context. |
| `code.ast` / `code.lsp` | **P1.5/P2 tech slice** | Now unblocked by the native cutover proof, but keep it out of the critical companion baseline unless structured code edits have a concrete user. |

### Parallelization seams

- Generated SDK docs/type maintenance can proceed independently of P2 work, as long as it reads from
  the existing host registry contract.
- Native coding-backend hardening can proceed independently of the P2 companion baseline as long as
  both preserve the same approval/resource boundaries.
- After actor mailbox semantics are fixed, supervision and `agent://` resource handling can split.

### Do-not-start-yet list

- Self-evolution writes before audit/replay and tier gates exist.
- Live external research before egress hardening, byte caps, audit logging, and secret handling are real.

## Crate plan

Current crates/apps: `tm-core`, `tm-llm`, `tm-sandbox`, `tm-artifacts`, `tm-host`, `tm-modes`,
`tm-agents`, `tm-memory`, `tm-drive`, `tm-server`, `apps/tm-cli`, and client scaffolds under `clients/`. The P0a bridge
currently lives in `tm-server::omp_acp`; P2 minimal memory still lives in `tm-server::memory` while
P4 dream queue ownership lives in `tm-memory`; `fs.*` /
`code.*` / `proc.*` live in `tm-host`; actor lifecycle, mailbox, orchestration, and resources live
in `tm-agents`; local-first drive storage, transducers, organizer proposals, links, and search feeds
live in `tm-drive`.

Planned product/support crates remain: full `tm-memory` (multi-mechanism recall + dreaming),
`tm-mcp`, and `tm-trace`. Existing core crates (§10.1) keep their contracts; future
work should extract product storage/orchestration only when the second concrete user exists.

## Deferred choices outside the audit

The audit contracts above have no open implementation decisions. These later product/packaging
choices did not block the closed audit hardening gate:

- **Deployment mode** — foreground CLI, launchd service, Docker, or lumo service packaging.
- **Drive sync** — local-first default (offline-first); optional CRDT-based cloud sync later (P5, §24.5).

See also §15 and §29.
