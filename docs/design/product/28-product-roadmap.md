# 28. Product roadmap

This is a **rewrite** (§29): reach behavioral parity with the current `hermes-agent`, then deepen.
Dogfood the **character** first. The runtime-core roadmap (§14, M0–M4) supplies prerequisites.

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

## Crate plan

New: `tm-server` (+ scheduler), `tm-persona`, `tm-memory` (multi-mechanism recall + dreaming), `tm-agents`,
`tm-drive` (+ `code.*` / `fs.*` / `proc.*` host fns in `tm-host`), clients (`web`, `android`).
Existing core crates (§10.1) keep their contracts; `tm-llm` gains model-role resolution (§27.3);
`tm-artifacts` extends to the two-tier model (§25.3).

## Open questions (character is now resolved — §21.5)

- **WebUI / Android stack** — P0 / P5.
- **Server API shape** — P0 (§27.5).
- **Drive sync** — local-first default (offline-first); optional CRDT-based cloud sync later (P4, §24.5).
- **`proc.run` allowlist** — which build/test commands per linked project (P1, §25.2).

See also §15 and §29.
