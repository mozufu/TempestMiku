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
| **P3 — handoff + orchestration** | `agents.*` (§23): actor sessions + message passing + supervision; Handoff mode + brief template | 5 | M2 | code fans out N subtasks + merges (only digest hits context); siblings coordinate via messages; a crashing sub-agent is isolated/restarted by its supervisor; handoff brief matches `oh-my-pi-handoff` |
| **P4 — dreaming + proactivity** | consolidation + `skills/` generation (§22/§26); scheduler/cron (§27.2) | all | — | post-session dream writes summary + ≥1 self-verified skill (approval-gated); weekly ship ledger runs on cron |
| **P5 — drive + research** | `drive.*` + auto-organizer (§24): transducers + virtual dirs; deep-research workspace (P3+P4+P5) | 4/5 | — | dropped file auto-filed by derived attributes; sandbox-default; a link mints `FsPolicy` + per-project memory scope; local-first offline |
| **P6 — Android package + OS integrations** | Ship the Android target of the existing Flutter client after the server API and product surfaces stabilize | all | — | Android + Web/PWA both connect to the same server/session; Android adds native packaging, secure pairing storage, push approval notifications, app links, and reconnect/resume polish without a second execution path |
| **P7 — self-evolution tiers + hardening** | tiers (§26); isolation / egress hardening | all | M3–M4 | tier switch changes write-scope; conservative writes only memory+skills; audit trail |

**Parity gate (§29.5):** P0–P4 are not "done" until they reproduce the current behavior for their
slice (coding reach, project continuity, voice, mode router, memory recall, approvals). New capability layers on *after* parity.

## Execution plan from current workspace (2026-07-01)

Current implementation has advanced past the original M0-only substrate. The workspace now includes
`tm-core`, `tm-llm`, `tm-sandbox` with both `StubSandbox` and a `deno_core` backend, `tm-artifacts`,
`tm-host`, `tm-persona`, `tm-server`, `apps/tm-cli`, and client scaffolds under `clients/`. The
implemented path covers M0, M1, host/approval/resource foundations, P0a OMP ACP bridging, the native
P0 Serious Engineer dogfood slice, the CLI native cutover proof for linked-repo edit/run/artifact
workflows, native Deno HTTP approvals for the Serious Engineer backend, the P1 project-manager
remote-control surface, and the P2 personal-assistant baseline. P2 now covers SOUL + active skill
bundles, bounded recall injection, approval-gated profile/recall/reminder writes, memory resources,
negative-state grounding guardrails, and gated Postgres coverage for the approval flow. Normal Rust
tests pass when local loopback networking is available; managed sandboxes may need elevated
permissions for tests that bind `127.0.0.1`.

Still not production-complete: `tm-mcp`, `tm-trace`, dedicated `tm-memory`, `tm-agents`, and
`tm-drive` crates; scheduler/dreaming; product approval surfaces for future drive/skill writes and
richer memory APIs; Android OS packaging; generated SDK docs from the runtime registry; and production
egress/secret hardening.

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
| 6 | **P3 — handoff + sub-agent actors** | 6–10d | Add `tm-agents` actor lifecycle, mailbox, roster, `agents.run/spawn/parallel/msg`, `agent://` resources, supervision defaults; implement Handoff brief template. | Fan-out N workers, only digest returns to parent context; siblings coordinate by messages; child crash isolated/restarted/degraded; handoff matches `oh-my-pi-handoff`. |
| 7 | **P4 — dreaming + scheduler** | 6–10d | Expand `tm-memory` to Postgres/pgvector + FTS; async episodic writes; dream queue; extraction, reflection, summaries, skill proposal; add cron scheduler. | Session end enqueues dream; dream writes summary + at least one approval-gated skill proposal; weekly ship ledger runs under cron bounds. |
| 8 | **P5 — drive + research workspace** | 5–9d | Add `tm-drive`, `drive.*`, local-first file metadata, transducers, virtual dirs, project memory scopes, drive organizer. | Dropped file auto-files by derived attributes after approval; project link mints `FsPolicy` + memory scope; offline local-first path works. |
| 9 | **P6 — Android package + OS integrations** | 4–7d | Package the existing Flutter client for Android; add mobile OS integrations such as secure pairing storage, push approval notifications, app links, and reconnect/resume polish. No on-device sandbox. | Android and Web/PWA attach to the same server/session stream; the Android build reuses the same chat, approval, artifact/resource, memory, drive, and agent-browser components rather than duplicating client code. |
| 10 | **P7 — self-evolution + hardening** | 6–12d | Implement evolution tiers; write-scope switches; audit trail; MCP import/reload gates; egress/isolation hardening; add replay checks. | Tier switch changes allowed writes; conservative writes only memory+skills proposals; audit/replay can explain every gated write. |

### Immediate next task queue

1. **Start P3 handoff + sub-agent actors** — add the `tm-agents` actor lifecycle, mailbox, roster,
   `agents.run/spawn/parallel/msg`, `agent://` resources, supervision defaults, and the Handoff brief
   template while keeping only digests in the parent context.
2. **Keep the native/OMP coding backend boundary boring** — OMP ACP remains replaceable, while the
   native Deno Serious Engineer backend is the dogfood path for `fs.*` / `code.*` / `proc.*`,
   artifacts, and HTTP-routed manual approvals.
3. **Defer expansion crates until the P3 actor surface is stable** — `tm-memory`,
   `tm-drive`, `tm-mcp`, scheduler/dreaming, and Android OS packaging should build on the settled
   server/resource surface.

### Deferred SDK namespace placement

These are roadmap-owned deferred tasks, not loose TODOs:

| Namespace / surface | Target milestone | Placement note |
|---|---|---|
| `memory.*` | **P2/P4 split** | P2 may expose the minimum profile/user recall and personal-assistant state-capture surface; P4 owns full `tm-memory`, pgvector/FTS, dream queue writes, and richer scoped memory APIs. |
| `agents.*` | **P3** | Lands with `tm-agents`, actor lifecycle, mailbox/roster, supervision, `agent://` resources, and Handoff mode. |
| `skills.*` | **P4/P7 split** | P4 can emit approval-gated skill proposals from dreaming; P7 owns safe import/version/reload semantics, provenance, audit/replay, and MCP import gates. |
| `drive.*` | **P5** | Lands with `tm-drive`, virtual dirs, transducers, project memory scopes, and drive organizer flows. |
| `http.*` hardening | **P5 or P7** | If deep research needs live egress, add byte/request caps, redirect policy, audit logging, and production allowlists in P5; otherwise keep `http.get` as the deterministic allowlist helper until P7 hardening. |
| `secrets.use` | **P7** | Requires an opaque-handle secret broker, egress-scoped grants, and audit guarantees that never materialize secret values in JS heap, artifacts, or model context. |
| `code.ast` / `code.lsp` | **P1.5/P2 tech slice** | Now unblocked by the native cutover proof, but keep it out of the critical companion baseline unless structured code edits have a concrete user. |

### Parallelization seams

- Generated SDK docs/type maintenance can proceed independently of P2 work, as long as it reads from
  the existing host registry contract.
- Native coding-backend hardening can proceed independently of the P2 companion baseline as long as
  both preserve the same approval/resource boundaries.
- After actor mailbox semantics are fixed, supervision and `agent://` resource handling can split.

### Do-not-start-yet list

- Android OS packaging before the P2/P3 product surfaces stabilize.
- Drive auto-organizer before approval policy and memory scopes exist.
- Self-evolution writes before audit/replay and tier gates exist.
- Research/deep orchestration before `agents.*`, artifacts, memory scopes, and scheduler are real.

## Crate plan

Current crates/apps: `tm-core`, `tm-llm`, `tm-sandbox`, `tm-artifacts`, `tm-host`, `tm-persona`,
`tm-server`, `apps/tm-cli`, and client scaffolds under `clients/`. The P0a bridge currently lives in
`tm-server::omp_acp`; minimal memory lives in `tm-server::memory`; `fs.*` / `code.*` / `proc.*` live
in `tm-host`.

Planned product/support crates remain: `tm-memory` (multi-mechanism recall + dreaming),
`tm-agents`, `tm-drive`, `tm-mcp`, and `tm-trace`. Existing core crates (§10.1) keep their
contracts; future work should extract product storage/orchestration only when the second concrete
user exists.

## Open questions (near-term resolved; deployment still open)

- **Deployment mode** — foreground CLI, launchd service, Docker, or lumo service packaging.
- **Drive sync** — local-first default (offline-first); optional CRDT-based cloud sync later (P5, §24.5).

See also §15 and §29.
