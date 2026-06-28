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
| **P0 — coding-agent dogfood** | Serious Engineer slice for TempestMiku itself: `fs.*`/`code.*`/`proc.*` (§25), linked repo, artifacts, minimal project memory, streaming CLI | 4 | M0–M2 | real linked-repo patch edit + targeted test succeeds; output spills to `artifact://`; project summary/open-loop memory is recalled next session; approvals gate destructive/out-of-grant ops; serious voice cap is 關 |
| **P1 — project manager + remote control** | Chief-of-staff layer over the coding loop: mode router, project plans/open loops/decisions, session event log, SSE/API project surface, project promotion, and Flutter Web/PWA remote-control client | 1/2/4 | M1–M2 | router fires; user can lock; badge + voice cap work; project status survives reconnect; Miku can turn a coding session into next actions without losing provenance; a phone/browser can attach to the same session stream, send a message, resolve an approval, open resource links, promote session workspace/artifacts into `project://`, and resume via `Last-Event-ID` |
| **P2 — personal assistant** | Full companion baseline: Miku voice, profile/user memory, personal-assistant state capture, negative-state grounding, bounded proactivity | 1–3 | M1–M2 | replies in Miku voice when appropriate; memory context + user profile load; personal reminders/open loops are captured with approval; grounding mode preserves the health-over-productivity rule |
| **P3 — handoff + orchestration** | `agents.*` (§23): actor sessions + message passing + supervision; Handoff mode + brief template | 5 | M2 | code fans out N subtasks + merges (only digest hits context); siblings coordinate via messages; a crashing sub-agent is isolated/restarted by its supervisor; handoff brief matches `oh-my-pi-handoff` |
| **P4 — dreaming + proactivity** | consolidation + `skills/` generation (§22/§26); scheduler/cron (§27.2) | all | — | post-session dream writes summary + ≥1 self-verified skill (approval-gated); weekly ship ledger runs on cron |
| **P5 — drive + research** | `drive.*` + auto-organizer (§24): transducers + virtual dirs; deep-research workspace (P3+P4+P5) | 4/5 | — | dropped file auto-filed by derived attributes; sandbox-default; a link mints `FsPolicy` + per-project memory scope; local-first offline |
| **P6 — Android package + OS integrations** | Ship the Android target of the existing Flutter client after the server API and product surfaces stabilize | all | — | Android + Web/PWA both connect to the same server/session; Android adds native packaging, secure pairing storage, push approval notifications, app links, and reconnect/resume polish without a second execution path |
| **P7 — self-evolution tiers + hardening** | tiers (§26); isolation / egress hardening | all | M3–M4 | tier switch changes write-scope; conservative writes only memory+skills; audit trail |

**Parity gate (§29.5):** P0–P4 are not "done" until they reproduce the current behavior for their
slice (coding reach, project continuity, voice, mode router, memory recall, approvals). New capability layers on *after* parity.

## Execution plan from current workspace (2026-06-27)

Current implementation is **core M0-shaped, not product-ready**: `tm-core`, `tm-llm`, `tm-sandbox`,
and `tm-cli` exist; `tm-sandbox` is still the M0 stub; product crates (`tm-server`, `tm-persona`,
`tm-memory`, `tm-agents`, `tm-drive`) and core support crates (`tm-host`, `tm-artifacts`, `tm-mcp`,
`tm-trace`) are not yet materialized. Plan from the substrate up; do not start product polish before
the real REPL and host/artifact spine exist.

Sizing below is **solo focused engineering days**. Treat it as sequencing pressure, not a promise:
each milestone is done only when its acceptance checks pass.

| Order | Milestone | Estimate | Tasks | Acceptance |
|---:|---|---:|---|---|
| 0 | **M0 closeout — streaming skeleton locked** | 0.5–1d | Freeze stream event contract; keep OpenAI SSE adapter tests; keep CLI token streaming; document current stub-sandbox boundary. | `cargo test`; scripted stream test proves token deltas, tool-call assembly, stub eval, and final answer path. |
| 1 | **M1 — real REPL + SDK spine** | 4–7d | Add `deno_core` backend behind `Sandbox`; implement persistent cells, cancel/reset, `display`, `http.get` allowlist, artifact writes; add `tm-artifacts` spill store; enforce `CellBudget` and output caps. | Rust tests cover persistent state, display/artifact capture, blocked network, timeout/cancel, shaped output truncation + artifact spill. |
| 2 | **Host registry + approvals foundation** | 3–5d | Add `tm-host`; define `HostFn`, capability grants, `ResourceRegistry`, `ApprovalPolicy`; wire SDK dispatch from sandbox ops; fail closed on unknown schemes/capabilities. | Unit tests prove denied capability cannot execute, approval timeout denies by default, `artifact://` resolves through the registry. |
| 3 | **P0 — coding-agent dogfood** | 5–8d | Add config-declared linked-folder `FsPolicy`; implement `fs.*`, search, patch-only `code.edit`, `proc.run(cmd,args)` allowlist, artifact spill, minimal project memory, and a streaming CLI surface for Serious Engineer mode. | Real TempestMiku patch edit+targeted `cargo test` succeeds through SDK; non-allowlisted commands fail closed; destructive/external/out-of-grant ops are approval-gated; output spills to `artifact://`; next session recalls the project summary/open loop. |
| 4 | **P1 — project manager + remote control** | 4–7d | Add `tm-server` (`axum`) custom session API + SSE, Postgres-backed session events/messages/project notes, mode router lock/override, `ModeChanged` events, project/open-loop views, approval resolution endpoints, session-scoped resource gateway, session-to-project promotion, and a Flutter client targeting Web/PWA first for mobile-ready remote control. | Reconnect replays from Postgres event id; router fires; user can lock; project status and decisions survive across sessions; `POST /sessions/:id/promote` turns workspace files/artifacts/summary/open loops into `project://` resources with provenance; a phone/browser can send a task, watch streaming events, approve/deny a gated action, open `workspace://` / `artifact://` / `project://` links through the gateway, disconnect, and resume without losing state. |
| 5 | **P2 — personal assistant** | 5–8d | Add full persona overlay, configurable SOUL / skills asset path with degraded boot warning, profile/user recall, personal-assistant state capture, negative-state grounding, and bounded proactive reminders. | Token-by-token Miku response over SSE; profile/recall context injected; voice is characterful outside serious mode; memory writes are approval-gated; health-over-productivity and proactivity bounds hold. |
| 6 | **P3 — handoff + sub-agent actors** | 6–10d | Add `tm-agents` actor lifecycle, mailbox, roster, `agents.run/spawn/parallel/msg`, `agent://` resources, supervision defaults; implement Handoff brief template. | Fan-out N workers, only digest returns to parent context; siblings coordinate by messages; child crash isolated/restarted/degraded; handoff matches `oh-my-pi-handoff`. |
| 7 | **P4 — dreaming + scheduler** | 6–10d | Expand `tm-memory` to Postgres/pgvector + FTS; async episodic writes; dream queue; extraction, reflection, summaries, skill proposal; add cron scheduler. | Session end enqueues dream; dream writes summary + at least one approval-gated skill proposal; weekly ship ledger runs under cron bounds. |
| 8 | **P5 — drive + research workspace** | 5–9d | Add `tm-drive`, `drive.*`, local-first file metadata, transducers, virtual dirs, project memory scopes, drive organizer. | Dropped file auto-files by derived attributes after approval; project link mints `FsPolicy` + memory scope; offline local-first path works. |
| 9 | **P6 — Android package + OS integrations** | 4–7d | Package the existing Flutter client for Android; add mobile OS integrations such as secure pairing storage, push approval notifications, app links, and reconnect/resume polish. No on-device sandbox. | Android and Web/PWA attach to the same server/session stream; the Android build reuses the same chat, approval, artifact/resource, memory, drive, and agent-browser components rather than duplicating client code. |
| 10 | **P7 — self-evolution + hardening** | 6–12d | Implement evolution tiers; write-scope switches; audit trail; MCP import/reload gates; egress/isolation hardening; add replay checks. | Tier switch changes allowed writes; conservative writes only memory+skills proposals; audit/replay can explain every gated write. |

### Immediate next task queue

1. **Close M0 with regression tests** — keep the current CLI + OpenAI-compatible stream skeleton stable
   before adding runtime complexity.
2. **Implement `tm-artifacts` before broad host SDK work** — output spill and `artifact://` are needed by
   REPL, agents, memory, and server replay.
3. **Implement real `deno_core` sandbox before P0 product work** — P0 dogfood should run on the same
   execution substrate the product will keep, not a special chat-only path.
4. **Implement `tm-host` + approvals before linked-folder engineering** — `proc.run` and file edits must
   be capability-gated from the first real repo operation.
5. **Ship P0 as a coding-agent vertical slice** — linked TempestMiku repo, patch edit, targeted test,
   artifact spill, minimal project memory, and serious-mode Miku voice together.

### Parallelization seams

- After `tm-artifacts` interfaces are fixed, `tm-server` SSE work and `tm-persona` asset loading can run
  in parallel with the `deno_core` backend.
- After `tm-host` grants are fixed, `fs.*` / `code.*` / `proc.run` can split across independent workers.
- After actor mailbox semantics are fixed, supervision and `agent://` resource handling can split.

### Do-not-start-yet list

- Android OS packaging before P1 server APIs stabilize; phone/browser remote control itself belongs in P1 through the shared SSE + POST API and Flutter Web/PWA target.
- Drive auto-organizer before approval policy and memory scopes exist.
- Self-evolution writes before audit/replay and tier gates exist.
- Research/deep orchestration before `agents.*`, artifacts, memory scopes, and scheduler are real.

## Crate plan

New: `tm-server` (+ scheduler), `tm-persona`, `tm-memory` (multi-mechanism recall + dreaming), `tm-agents`,
`tm-drive` (+ `code.*` / `fs.*` / `proc.*` host fns in `tm-host`), and `clients/miku_flutter`
(single Flutter codebase targeting Web/PWA first and Android later). Existing core crates (§10.1) keep
their contracts; `tm-llm` gains model-role resolution (§27.3); `tm-artifacts` extends to the two-tier model (§25.3).

## Open questions (near-term resolved; deployment still open)

- **Deployment mode** — foreground CLI, launchd service, Docker, or lumo service packaging.
- **Drive sync** — local-first default (offline-first); optional CRDT-based cloud sync later (P5, §24.5).

See also §15 and §29.
