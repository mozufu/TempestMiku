# TODO

Last aligned: **2026-07-08**.

Active milestone: **P4 — dreaming + proactivity**.

`ROADMAP.md` remains the canonical milestone order. This file is the working execution checklist for
finishing P4 after the P3-plus actor/mailbox/supervision/provenance closeout and the landed P4.0
dream-queue slice. Keep it aligned with core docs §07/§09 and product docs §21, §22, §26, §27, and
§29.

## P4 North Star

Ship a replayable, approval-bound dreaming and scheduler loop:

- Ending a session enqueues durable dream work.
- A worker leases queued dreams, extracts useful memory, summarizes what happened, reflects on
  reusable lessons, and proposes at least one self-verified skill when the session contains a stable
  recurring workflow.
- Durable facts, summaries, and skill proposals are redacted, deduped, provenance-linked, and
  approval-gated where the docs require it.
- The weekly ship ledger runs from cron under proactivity bounds, streams through the same SSE/event
  log, and defers on approvals instead of auto-acting.

P4 is not a drive, MCP, Android, or self-rewrite milestone. It extends the existing server, store,
resource, approval, mode, and actor contracts; it does not fork a second orchestration path.

## Non-Negotiable Invariants

- The model still gets one chat-native tool: `execute(code)`.
- Streaming remains the source of truth. Dream, approval, and scheduler progress must be represented
  as replayable `session_events` and SSE frames.
- Capabilities are config and grants, not prompts. Unknown capability names and resource schemes fail
  closed.
- `skill://<name>` is prompt-composition-only until a P4/P7-approved read/preview surface lands with
  grants, provenance, and tests.
- `skills.*` import/version/reload remains P7. P4 may produce approval-gated skill proposals, but it
  must not silently install or reload skills.
- `memory://` resources remain the public read surface for existing P2 memory records. Any P4
  `memory.*` API must ship with host-catalog docs, SDK typings, grant tests, and denial behavior.
- Scheduled proactivity honors §21.3: safe summarize/organize/review work may run; destructive,
  external, commitment, spend, sensitive, or publish/send actions defer through approvals.
- Serious Engineer, Handoff, and child-agent approval behavior from P0-P3+ must not regress.
- Normal `cargo test` stays external-service-free. Live LLM, Postgres, and browser/Flutter coverage
  stay opt-in or gated.

## Current Baseline

- [x] P0/P1/P2 are complete per `ROADMAP.md`.
- [x] P3 and P3-plus are complete: `agents.run/spawn/parallel/msg/send/broadcast/wait/inbox/list/cancel/pipeline`,
      live inboxes, active supervision, child approvals, replayable actor/resource provenance, and
      Flutter/tm-e2e actor smoke coverage are in place.
- [x] Persona assets are catalog-driven from `SOUL.md`, `modes.json`, and bundled/configured
      `skills/<name>/SKILL.md`.
- [x] P2 memory context injects bounded profile facts and scoped recall with provenance.
- [x] P2 memory writes use approval-gated `write_proposal` flows; deny/timeout writes nothing.
- [x] `memory://` resources expose the current P2 profile/user-model and approved record views.
- [x] P4.0: `tm-memory` owns `DreamQueueRecord`, `DreamReason`, `DreamStatus`,
      `NewDreamQueueRecord`, `DreamWorker`, `DreamWorkerReport`, and `NoopDreamWorker`.
- [x] P4.0: `tm-server` persists `dream_queue` rows in memory/Postgres stores.
- [x] P4.0: `POST /sessions/:id/end` marks the session ended, enqueues one idempotent dream record,
      and emits replayable `session_end` + `dream_queued` events.
- [x] P4.1/P4.7 store spine: in-memory/Postgres stores include dream lifecycle operations,
      `memory_summaries`, `skill_proposals`, `cron_jobs`, and `cron_runs`.
- [x] P4 deterministic worker: `ServerDreamWorker` leases dreams, redacts input, writes bounded
      session summaries, emits approval-gated memory/skill proposals, and records replayable dream
      lifecycle events without external services.
- [x] P4 scheduler slice: weekly ship ledger can run through a normal session under
      `cron_mode: deny`, `max_turns <= 8`, and a 120-second script bound; `cron://` resources expose
      job/run previews.

## P4 Acceptance Gate

- [x] Post-session dream writes a bounded session summary with evidence/provenance links.
- [x] Post-session dream extracts durable memory candidates, applies redaction/dedup rules, and uses
      existing approval/default-deny semantics before writing durable facts or scoped recall chunks.
- [x] Post-session dream emits at least one self-verified skill proposal when the session contains a
      reusable workflow; the proposal is approval-gated and not silently installed.
- [x] Weekly ship ledger runs from cron under `cron_mode: deny`, `goals.max_turns <= 8`, and the
      120-second script bound; approval-needed actions are deferred, not auto-applied.
- [x] Dream and cron runs are visible through the same event log/SSE replay path as interactive
      sessions.
- [x] Parity §29.5 still holds: Miku voice, mode router, memory recall/profile, manual approvals,
      project continuity, and Serious Engineer coding reach are preserved.
- [x] Normal `cargo test` passes without external services; gated Postgres/live/browser tests document
      their env vars and stay opt-in.

## P4.1 Store And Migration Spine

- [x] Move more P2 memory record ownership from `tm-server::memory` toward `tm-memory` only where
      there is a concrete P4 worker/retrieval user; avoid churny extraction for its own sake.
- [x] Define `tm-memory::store` traits for the P4 logical stores needed now:
      episodic messages/events, summaries, profile/fact candidates, scoped recall chunks, skill
      proposals, and dream leases.
- [x] Extend `Store` with dream-queue worker operations:
      `claim_ready_dream`, `heartbeat_dream`, `complete_dream`, `fail_dream`, and retry/backoff
      metadata.
- [x] Preserve `enqueue_dream` idempotency by `dedupe_key`; duplicate session-end calls must not
      duplicate work or events.
- [x] Add in-memory store support for the full worker lifecycle so unit tests remain external-free.
- [x] Add Postgres schema additions behind gated tests:
      `memory_summaries`, `skill_proposals`, and any normalized P4 worker tables required before
      pgvector/FTS.
- [x] Add ready-queue indexes for `(status, available_at)`, stale-lock recovery, and session/scope
      queries.
- [x] Add JSON wire tests for every new event/resource payload before exposing it to clients.
- [x] Add gated Postgres tests for dream claim/complete/fail idempotency, stale lock recovery, and
      duplicate enqueue behavior.

Acceptance:

- [x] A queued dream can be claimed exactly once, heartbeated, completed, and replayed using both
      in-memory and Postgres stores.
- [x] A failed dream records `attempts`, `last_error`, and next `available_at`; retries are bounded and
      deterministic.
- [x] Normal tests do not require Postgres.

## P4.2 Dream Worker Runtime

- [x] Replace `NoopDreamWorker` usage with a real worker implementation while keeping the trait usable
      in tests.
- [x] Add a server-owned worker runner that can run once in tests and loop in daemon mode without
      blocking request handling.
- [x] Ensure every worker state transition emits replayable events:
      `dream_started`, `dream_progress`, `dream_completed`, `dream_failed`, and proposal events where
      applicable.
- [x] Add a worker config block for enable/disable, poll interval, lease timeout, max attempts,
      concurrency, per-dream timeout, and model-role names.
- [x] Wire cancellation/shutdown so the server can stop cleanly without leaving permanent locks.
- [x] Make worker output bounded. Large summaries, extracted evidence, or verification logs should
      spill to `artifact://` or resource rows, not into model context.
- [x] Add scripted test workers/LLM clients for extraction, summary, critique, and error cases.
- [x] Add failure-mode tests for model error, timeout, redaction failure, approval timeout, store
      failure, and duplicate worker race.

Acceptance:

- [x] `DreamWorker::run_once` can process one ready record end-to-end in a deterministic test.
- [x] Two concurrent workers cannot complete the same dream twice.
- [x] Worker failure is replayable and leaves the queue retryable or terminal according to config.

## P4.3 Episodic Capture And Redaction

- [x] Build the dream input collector from the existing session tables/event log:
      messages, final answer, mode changes, approvals, memory proposals, actor digests, project
      promotions, artifact/resource links, and session-end metadata.
- [x] Keep child-agent transcripts out of dream prompt context by default; include `agent://` /
      `history://` / `artifact://` handles and bounded previews only when useful.
- [x] Add deterministic redaction before any dream prompt or durable derived write:
      secrets/tokens, obvious credentials, private keys, raw auth headers, and sensitive PII patterns.
- [x] Preserve negative-state discipline: distress-only sessions may summarize supportively but must
      not create durable profile facts unless Brian explicitly asked to remember stable signal.
- [x] Attach provenance to every candidate: source session id, event sequence(s), message ids,
      resource URIs, scope, and subject.
- [x] Add a compact prompt budgeter so long sessions are chunked before LLM extraction.
- [x] Add tests for redaction, bounded previews, provenance, and negative-state no-write behavior.

Acceptance:

- [x] Dream input is sufficient for extraction but never contains unbounded transcripts or obvious
      secrets.
- [x] Every candidate durable write can be traced back to exact session/event/resource evidence.

## P4.4 Extraction, Importance, And Memory Writes

- [x] Implement extraction passes for the P4-worthy durable memory classes:
      stable preferences, commitments/deadlines, decisions, shipped artifacts, reusable workflows,
      project open loops, and recurring blind spots.
- [x] Score importance/poignancy for extracted candidates and store the score with provenance.
- [x] Add dedupe and contradiction handling:
      normalize candidate text, merge duplicates, supersede obsolete facts with `valid_to`, never
      erase history.
- [x] Keep project-specific commands and repo lore scoped to the active project scope, not global user
      memory.
- [x] Use the existing `write_proposal` + `approval` route for durable fact/chunk writes unless config
      explicitly grants safe auto-write for the record class.
- [x] Ensure approval deny/timeout writes nothing and emits resolved proposal status.
- [x] Add exact `memory://` resource previews for new summary/fact/chunk records.
- [x] Add tests for approve, deny, timeout/default-deny, idempotent re-run, contradiction supersede,
      scope isolation, and provenance replay.

Acceptance:

- [x] A dream can propose and, after approval, persist durable facts/chunks without blocking normal
      chat turns.
- [x] Re-running the same dream does not duplicate facts, chunks, or approvals.
- [x] Sensitive or transient content is skipped before proposal creation.

## P4.5 Summaries, Reflection, And Recall

- [x] Add summary record types for session, daily, weekly, and topic/project rollups.
- [x] Implement the first session-summary dream output:
      what happened, shipped artifacts, decisions, open loops, unresolved approvals, and next likely
      actions.
- [x] Add reflection synthesis when cumulative importance crosses a threshold; reflections must cite
      evidence and be stored as derived memory, not raw assertion.
- [x] Add recursive summary update logic so old sessions can be folded without loading full logs.
- [x] Extend recall to use P4 summaries alongside P2 profile facts and scoped recall chunks.
- [x] Add a hybrid retrieval stepping-stone before full pgvector:
      exact/FTS-style lexical query plus scoped recency and importance scoring.
- [x] Add Postgres FTS coverage when the Postgres gate is enabled; keep in-memory fallback deterministic.
- [x] Defer dense embeddings/pgvector until the lexical + summary path is stable, then add provider
      config and dimension pinning.
- [x] Add tests that session-start memory context can include a recent summary with provenance and
      bounded token budget.

Acceptance:

- [x] Ending a productive session creates a reusable summary visible through `memory://`.
- [x] A later session in the same scope can recall the summary/open loops without loading raw logs.
- [x] Recall degrades gracefully when optional vector/FTS stores are empty or unavailable.

## P4.6 Skill Proposal Generation

- [x] Define `SkillProposal` data in `tm-memory`:
      id, name, description, body, trigger/use criteria, evidence, self-critique, verification result,
      status, dedupe key, created/updated timestamps, and source dream id.
- [x] Distill skill candidates only from repeated or clearly reusable workflows, not one-off commands
      or project-local trivia.
- [x] Generate skill bodies in the existing `SKILL.md` style with concise trigger rules and procedural
      steps.
- [x] Add self-critique/refine pass before surfacing a proposal.
- [x] Add self-verification:
      dry-run the skill against the source scenario, check frontmatter/body shape, check no forbidden
      capability claims, and ensure it does not mutate `SOUL.md`, modes, or grants.
- [x] Emit `write_proposal` with `kind: "skill"` and use the shared approval/default-deny flow.
- [x] Store approved proposals as pending skill assets or reviewable files according to the eventual
      P4/P7 split; do not install/reload them into the live prompt catalog automatically.
- [x] Add `skill_proposal://` or a constrained `memory://.../skill-proposals/...` preview resource
      before exposing full `skill://` reads.
- [x] Add tests for low-value candidate rejection, self-critique refinement, verification failure,
      approval approve/deny/timeout, dedupe on re-run, and no live skill reload.

Acceptance:

- [x] At least one representative post-session dream produces a self-verified skill proposal.
- [x] Approval can accept or reject the proposal; rejection leaves the live skill catalog unchanged.
- [x] P7-owned `skills.*` import/version/reload remains unimplemented and fail-closed.

## P4.7 Scheduler And Cron

- [x] Add a scheduler module under `tm-server` with cron-style job definitions, next-run calculation,
      job state, run history, and per-job bounds.
- [x] Add persistent job/run tables to in-memory and Postgres stores.
- [x] Model reserved `cron://` resources:
      job list, job definition preview, run history, and last result.
- [x] Register `cron://` with the resource gateway only when grants/docs/tests are in place; until then
      reserved paths must keep failing closed.
- [x] Implement the weekly ship ledger job using the existing `weekly-ship-ledger` skill.
- [x] Start scheduled runs through the same session API/agent loop/event stream as interactive work.
- [x] Enforce `cron_mode: deny`: any approval-needed action in a scheduled run is deferred and
      surfaced for Brian; it is never auto-approved.
- [x] Enforce `goals.max_turns <= 8`, script timeout <= 120 seconds, bounded model role, and one-run
      per scheduled fire.
- [x] Handle offline/client-disconnected mode by logging queued/pending approvals for later review.
- [x] Add tests for cron parsing, missed-run policy, duplicate-fire prevention, approval deferral,
      bounds enforcement, event replay, and `cron://` resource previews.

Acceptance:

- [x] A deterministic weekly ship ledger run can be triggered in tests and emits a replayable session.
- [x] Scheduled runs share the normal approval, memory, artifact, and resource pathways.
- [x] Scheduler restart does not double-run completed or currently leased jobs.

## P4.8 Resource, API, And Client Surface

- [x] Add full dream inspection endpoints or resources:
      queue status and exact dream records are visible through `memory://dreams`; dream run events,
      produced summaries, and proposals are visible through `session_events` / `memory://`.
- [x] Prefer the session resource gateway for previews over bespoke debug routes.
- [x] Add event payload docs for all new P4 event types.
- [x] Update `docs/sdk/tm-runtime.d.ts` for any new resource URI shape or approved `memory.*` /
      scheduler surface in the same change as host docs/tests.
- [x] Add compact mobile-friendly payloads for pending memory/skill proposals and scheduled-run
      status.
- [x] Add Flutter/Web smoke coverage only after the server event/resource shape is stable:
      pending dream/proposal display, approval resolution, cron run replay, and resource open.
- [x] Keep clients thin; no client gains direct write authority over memory, skills, files, or cron.

Acceptance:

- [x] A browser/phone can see a dream or cron proposal, resolve it through the existing approval route,
      open the produced memory/summary/proposal resource, and reconnect via `Last-Event-ID`.

## P4.9 Model Roles And Configuration

- [x] Add config for model roles used by dreaming:
      extraction, reflection, summarization, skill distillation, self-critique, verification, and
      embeddings if/when dense recall lands.
- [x] Keep aux dream roles on cheaper/fallback-capable models by default.
- [x] Add config for redaction, token budgets, summary cadence, reflect threshold, retry/backoff, and
      scheduler bounds.
- [x] Ensure missing model config degrades visibly and safely:
      queue remains pending or failed with `last_error`, no partial unapproved writes.
- [x] Add docs for env vars and gated live tests.

Acceptance:

- [x] P4 can run deterministically with scripted model clients in normal tests and with real model
      roles only under explicit live-test configuration.

## P4.10 Documentation And Verification

- [x] Update §22 when storage/worker behavior changes:
      implemented tables, event names, failure modes, and recall behavior.
- [x] Update §26 when the skill proposal lifecycle is concretely implemented.
- [x] Update §27 when scheduler jobs, bounds, `cron://`, or client surfaces land.
- [x] Update `ROADMAP.md` only when P4 acceptance checks are actually satisfied.
- [x] Add focused unit tests before broad `cargo test`:
      dream queue lifecycle, extraction filters, redaction, summary write, skill proposal gate, and
      scheduler fire.
- [x] Run `cargo test` before marking any P4 slice complete.
- [x] Run gated Postgres tests for schema/lease/replay work:
      `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=postgres://... cargo test -p tm-server`.
- [x] Run tm-e2e/Flutter smoke only after public API/event shapes change and need client proof.
- [x] Keep live OpenAI tests opt-in and skipped by default.

## Deferred Namespace Placement

These are roadmap-owned deferred tasks, not loose TODOs:

| Namespace / surface | Target milestone | Placement note |
|---|---|---|
| `memory.*` | **P4, only when justified** | P2 uses `memory://` resources plus server-side write proposals. P4 may add `memory.recall` / `memory.reflect` only after grants, host docs, SDK typings, denial tests, and approval policy exist. |
| `skills.*` / full `skill://` reads | **P4/P7 split** | P4 produces approval-gated skill proposals and may expose constrained proposal previews. P7 owns import/version/reload, live catalog mutation, MCP import gates, and full audit/replay semantics. |
| `cron://` | **P4 ✓** | Session resource gateway exposes scheduler job definitions and run history with bounds and replay tests. Sandbox/runtime `cron.*` write authority remains absent. |
| `drive.*` | **P5** | Lands with `tm-drive`, virtual dirs, transducers, project memory scopes, and drive organizer flows. |
| `http.*` hardening | **P5 or P7** | If deep research needs live egress, add byte/request caps, redirect policy, audit logging, and production allowlists in P5; otherwise keep `http.get` as deterministic allowlist helper until P7 hardening. |
| `secrets.use` | **P7** | Requires an opaque-handle secret broker, egress-scoped grants, and audit guarantees that never materialize secret values in JS heap, artifacts, or model context. |
| `code.ast` / `code.lsp` | **Post-critical tech slice** | Useful for coding-agent quality, but keep it out of P4 unless a concrete dream/scheduler user requires it. |

## Parallelization Seams

- Dream queue leasing/store work can proceed independently from LLM extraction as long as the worker
  trait stays scripted-testable.
- Summary/recall can land before dense embeddings; lexical + recency + importance is enough for the
  first P4 acceptance path.
- Skill proposal generation can use stored summaries/evidence before full graph recall exists.
- Scheduler/job tables and the deterministic weekly ledger trigger are now in place; future scheduler
  work should focus on daemon looping, missed-run policy, restart duplicate prevention, and client
  smoke.
- Flutter smoke can wait until the server event/resource contract stabilizes.

## Do-Not-Start-Yet List

- Android OS packaging before P5/P6 server and resource surfaces stabilize.
- Drive auto-organizer before P5 approval policy and memory scopes are in place.
- `skills.*` live import/reload, MCP import gates, and tiered self-evolution writes before P7.
- Secret broker work and production egress hardening outside their hardening milestone unless a P4
  acceptance test truly requires a small prerequisite.
- Research/deep orchestration beyond the existing P3+ actor surface before memory scopes, scheduler,
  and drive surfaces are real.
- Any relaxation of manual approval, unknown-scheme fail-closed behavior, argv-vector `proc.run`, or
  linked-folder grant boundaries.

## Crate Plan

Current crates/apps: `tm-core`, `tm-llm`, `tm-sandbox`, `tm-artifacts`, `tm-host`, `tm-modes`,
`tm-agents`, `tm-memory`, `tm-server`, `apps/tm-cli`, `apps/tm-e2e`, and client scaffolds under
`clients/`.

P4 ownership:

- `tm-memory`: dream queue, worker contracts, profile/recall record shapes,
  extraction/summary/proposal data types, input budgeting, recall logic, redaction, future
  embeddings/FTS/graph modules.
- `tm-server`: durable store implementations, session/dream/scheduler APIs, event log/SSE emission,
  approval broker integration, resource registration, model-role wiring, daemon worker loop.
- `tm-modes`: existing skill assets and prompt catalog. P4 must not mutate the live catalog without
  the approved P4/P7 lifecycle.
- `tm-host` / `tm-sandbox`: only touched if P4 exposes a new capability-gated SDK namespace; update
  host docs, SDK typings, grants, and denial tests in the same change.
- `clients/miku_flutter` and `apps/tm-e2e`: smoke coverage for visible P4 events/resources after the
  server shape stabilizes.

Planned product/support crates remain: `tm-drive`, `tm-mcp`, and `tm-trace`. Extract new crates only
after a second concrete user exists.

## Open Questions

- Should P4 expose `memory.reflect()` to sandbox code, or keep manual reflection server-only until the
  dream worker is stable?
- Should approved skill proposals write to a configured review folder, a database row, or both before
  P7 import/version/reload exists?
- What is the first dense-embedding backend to support after lexical recall proves useful:
  OpenAI-compatible embeddings, local `fastembed`/`candle`, or both behind config?
- What missed-run policy should cron use on restart: run once immediately, skip missed windows, or
  queue a bounded catch-up?
- Which client surface should display pending dreams first: project view, session details, or a
  dedicated proactivity/review inbox?
