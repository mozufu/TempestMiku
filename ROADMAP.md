# TempestMiku roadmap

This is the canonical roadmap for TempestMiku. Design-doc section references remain stable:

- Core runtime roadmap: §14.
- Product roadmap and execution order: §28.

TempestMiku is a **rewrite** (§29): reach behavioral parity with the current `hermes-agent`, then deepen.
Dogfood the **character** first. The runtime-core roadmap (§14, M0–M4) supplies prerequisites.

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
| **P0 — character slice** | Miku chat over `tm-server` + WebUI, streaming over SSE (§27), memory recall | 1 | M0–M1 | replies **in Miku voice** (not mechanical); memory context + user profile load; token-by-token |
| **P1 — modes + engineer** | persona layer + 5-mode router (§21); `fs.*`/`code.*`/`proc.*` (§25); linked folders | 1–4 | M1–M2 | router fires; user can lock; badge + voice cap (關 when serious); real-repo edit+test via linked folder; approvals gate destructive ops |
| **P2 — handoff + orchestration** | `agents.*` (§23): actor sessions + message passing + supervision; Handoff mode + brief template | 5 | M2 | code fans out N subtasks + merges (only digest hits context); siblings coordinate via messages; a crashing sub-agent is isolated/restarted by its supervisor; handoff brief matches `oh-my-pi-handoff` |
| **P3 — dreaming + proactivity** | consolidation + `skills/` generation (§22/§26); scheduler/cron (§27.2) | all | — | post-session dream writes summary + ≥1 self-verified skill (approval-gated); weekly ship ledger runs on cron |
| **P4 — drive + research** | `drive.*` + auto-organizer (§24): transducers + virtual dirs; deep-research workspace (P2+P3+P4) | 4/5 | — | dropped file auto-filed by derived attributes; sandbox-default; a link mints `FsPolicy` + per-project memory scope; local-first offline |
| **P5 — Android** | thin Android client (§27.4) | all | — | Android + WebUI both connect to the same server |
| **P6 — self-evolution tiers + hardening** | tiers (§26); isolation / egress hardening | all | M3–M4 | tier switch changes write-scope; conservative writes only memory+skills; audit trail |

**Parity gate (§29.5):** P0–P3 are not "done" until they reproduce the current behavior for their
slice (voice, mode router, memory recall, approvals). New capability layers on *after* parity.

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
| 3 | **P0 — character server slice** | 5–8d | Add `tm-server` (`axum`) custom session API + SSE; Postgres-backed sessions/events/messages/profile/recall; configurable SOUL / skills asset path with degraded boot warning; chat-only Vite/React WebUI first stream. | Browser/user smoke: token-by-token Miku response over SSE, default mode badge, profile/recall context injected, reconnect replays from Postgres event id; Playwright smoke passes. |
| 4 | **P1 — modes + serious engineer** | 5–8d | Implement mode router, lock/override, `ModeChanged` events; add config-declared linked-folder `FsPolicy`; implement `fs.*`, patch-only `code.edit` first, `proc.run(cmd,args)` allowlist with Pi-style yolo within configured grants. | Real linked repo patch edit+test succeeds through SDK; non-allowlisted commands fail closed; destructive/external/out-of-grant ops are approval-gated; serious mode voice cap is 關. |
| 5 | **P2 — handoff + sub-agent actors** | 6–10d | Add `tm-agents` actor lifecycle, mailbox, roster, `agents.run/spawn/parallel/msg`, `agent://` resources, supervision defaults; implement Handoff brief template. | Fan-out N workers, only digest returns to parent context; siblings coordinate by messages; child crash isolated/restarted/degraded; handoff matches `oh-my-pi-handoff`. |
| 6 | **P3 — dreaming + scheduler** | 6–10d | Expand `tm-memory` to Postgres/pgvector + FTS; async episodic writes; dream queue; extraction, reflection, summaries, skill proposal; add cron scheduler. | Session end enqueues dream; dream writes summary + at least one approval-gated skill proposal; weekly ship ledger runs under cron bounds. |
| 7 | **P4 — drive + research workspace** | 5–9d | Add `tm-drive`, `drive.*`, local-first file metadata, transducers, virtual dirs, project memory scopes, drive organizer. | Dropped file auto-files by derived attributes after approval; project link mints `FsPolicy` + memory scope; offline local-first path works. |
| 8 | **P5 — Android thin client** | 4–7d | Build Android chat/badge/browser client against existing server API; no on-device sandbox; shared auth/pairing. | Android and WebUI attach to the same server/session stream; reconnect/resume works on mobile. |
| 9 | **P6 — self-evolution + hardening** | 6–12d | Implement evolution tiers; write-scope switches; audit trail; MCP import/reload gates; egress/isolation hardening; add replay checks. | Tier switch changes allowed writes; conservative tier writes only memory/skills proposals; audit/replay can explain every gated write. |

### Immediate next task queue

1. **Close M0 with regression tests** — keep the current CLI + OpenAI-compatible stream skeleton stable
   before adding runtime complexity.
2. **Implement `tm-artifacts` before broad host SDK work** — output spill and `artifact://` are needed by
   REPL, agents, memory, and server replay.
3. **Implement real `deno_core` sandbox before P0 product work** — P0 dogfood should run on the same
   execution substrate the product will keep, not a special chat-only path.
4. **Implement `tm-host` + approvals before linked-folder engineering** — `proc.run` and file edits must
   be capability-gated from the first real repo operation.
5. **Ship P0 as one vertical slice** — server SSE, persona overlay, and minimal memory recall together;
   a mechanical chat server without Miku voice does not satisfy parity.

### Parallelization seams

- After `tm-artifacts` interfaces are fixed, `tm-server` SSE work and `tm-persona` asset loading can run
  in parallel with the `deno_core` backend.
- After `tm-host` grants are fixed, `fs.*` / `code.*` / `proc.run` can split across independent workers.
- After actor mailbox semantics are fixed, supervision and `agent://` resource handling can split.

### Do-not-start-yet list

- Android before P0/P1 server APIs stabilize.
- Drive auto-organizer before approval policy and memory scopes exist.
- Self-evolution writes before audit/replay and tier gates exist.
- Research/deep orchestration before `agents.*`, artifacts, memory scopes, and scheduler are real.

## Crate plan

New: `tm-server` (+ scheduler), `tm-persona`, `tm-memory` (multi-mechanism recall + dreaming), `tm-agents`,
`tm-drive` (+ `code.*` / `fs.*` / `proc.*` host fns in `tm-host`), clients (`web`, `android`).
Existing core crates (§10.1) keep their contracts; `tm-llm` gains model-role resolution (§27.3);
`tm-artifacts` extends to the two-tier model (§25.3).

## Open questions (near-term resolved; deployment still open)

- **Deployment mode** — foreground CLI, launchd service, Docker, or lumo service packaging.
- **Drive sync** — local-first default (offline-first); optional CRDT-based cloud sync later (P4, §24.5).

See also §15 and §29.
