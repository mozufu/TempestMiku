# TODO

Last aligned: **2026-07-04**.

Active milestone: **P3 - handoff + sub-agent actors**.
Preflight: finish the remaining catalog validation gap, then start the actor surface.

`ROADMAP.md` remains the canonical milestone order. This file is the working execution checklist for
the next slice after the P2 personal-assistant baseline. Keep it aligned with core docs §07/§09 and
product docs §21, §22, §23, §25, §26, §27, and §29.

P2 is closed in the roadmap. New work starts from the runtime catalog system now in the tree; do not
reintroduce hardcoded mode prompts, hidden capability grants, or chat-native tool sprawl.

## Catalog invariants

- The model still gets one chat-native tool: `execute(code)`.
- Runtime capability discovery is the host capability catalog exposed through
  `tools.search(...)`, `tools.docs(...)`, and `tools.call(...)`.
- Persona/routing discovery is the mode + skill catalog:
  - `SOUL.md` is the constant identity and boundary layer.
  - `modes.json` is the source of truth for runtime mode ids, labels, route triggers, default scope,
    voice cap, declared capabilities, and `activeSkills`.
  - Mode ids are loaded strings, not Rust enum variants.
  - `skills/<name>/SKILL.md` files are procedural assets loaded as `skill://<name>` prompt sections.
  - Skills are not new chat-native tools and do not grant host capabilities by themselves.
- First-pass reserved sandbox namespaces stay explicit: `secrets`, `memory`, `skills`, and `agents`
  are `undefined` until their owning milestones add grants, docs, tests, and policy.
  - P2 memory reads are `resources.read("memory://...")`, not a `memory.*` global.
  - P3 is the point where `agents.*` may become defined, but only in granted sessions.
  - `skills.*` remains reserved until the P4/P7 split owns proposal/import/version/reload policy,
    provenance, approval, and audit.
- `skill://<name>` is a first-class resource URI in the design docs. Today it is implemented for
  persona prompt composition only; broader read/import/reload semantics belong to the future skills
  store, and unregistered `skill://` resource reads fail closed through the unknown-scheme path.
- Mode-declared capabilities are catalog metadata and grant inputs. Unknown capability names, unknown
  resource schemes, and missing grants fail closed.

## Current baseline

- [x] P0/P1/P2 are complete per `ROADMAP.md`.
- [x] P0 Serious Engineer dogfood remains on the native Deno SDK path for `fs.*`, `code.*`,
      `proc.*`, artifacts, and HTTP-routed manual approvals; OMP ACP stays replaceable.
- [x] P1 project-manager and remote-control surfaces cover mode routing/locks, project views,
      session promotion, resource gateway access, approvals, and SSE replay.
- [x] Bundled persona assets include `SOUL.md`, `modes.json`, and the seven default skills under
      `crates/tm-persona/assets/`.
- [x] Configured persona assets can override the bundle through `TM_PERSONA_PATH`.
- [x] Missing or unreadable persona assets degrade with visible warnings and bundled fallbacks.
- [x] `ModeCatalog` / `ModeProfile` load runtime string ids from `modes.json`.
- [x] `GET /modes`, session responses, mode responses, and `ModeChanged` events expose
      `activeSkills`.
- [x] Per-turn prompts compose core runtime notes, `SOUL.md`, active mode profile, active
      `skill://<name>` markdown, runtime capability notes, and persona warnings in stable order.
- [x] P2 memory context injects bounded profile facts and scoped recall with provenance.
- [x] P2 memory writes are approval-gated `write_proposal` flows; approve writes durable records,
      deny/timeout writes nothing, and outcomes replay as events.
- [x] Personal-assistant state capture proposes stable preferences, reminders, open loops,
      commitments/deadlines, decisions, shipped artifacts, and workflows while skipping transient,
      sensitive, raw-log, and negative-state content by default.
- [x] Negative-State Grounding routes as a conversational posture, keeps health over productivity,
      and forbids unsolicited memory writes.
- [x] P2 memory reads are exposed through `memory://` resources; the `memory` global remains
      `undefined`.
- [x] `docs/sdk/tm-runtime.d.ts` includes `skill://` resource URI typing and keeps `skills`
      reserved.
- [x] Gated Postgres coverage exists for the P2 memory approval flow while normal `cargo test`
      stays external-service-free.

## P3 catalog preflight

- [x] Keep `docs/sdk/tm-runtime.d.ts` as a checked-in SDK snapshot verified by exhaustive
      `tools.docs` parity tests.
- [x] Enrich `tools.docs` entries so every available direct namespace method documents signature,
      schemas, examples, errors, grants, and approval policy.
- [x] Add coverage comparing `tools.docs` with `docs/sdk/tm-runtime.d.ts` for `tools`, `resources`,
      `artifacts`, `fs`, `code`, `proc`, and `http`.
- [x] Validate catalog skill references: every bundled `activeSkills` entry should resolve to
      `skills/<name>/SKILL.md`; configured missing skills should emit an exact warning and stable
      prompt fallback.
- [x] Keep the fixture proving a new mode can be added through `modes.json` without a Rust enum or
      callsite migration.
- [x] Document the configured persona catalog layout: `SOUL.md`, `modes.json`, and
      `skills/<name>/SKILL.md`.
- [x] Reconcile `http.get` docs and runtime behavior with the deferred production egress policy:
      deterministic allowlist helper now, hardened egress later.
- [x] Keep read-only `skill://<name>` resource reads deferred before P4/P7; for now `skill://` is
      prompt-composition-only and unregistered resource reads fail closed.
- [ ] Reconcile the P3 MVP SDK subset with §23: `ROADMAP.md` names `agents.run/spawn/parallel/msg`,
      while §23 also sketches `pipeline`, `broadcast`, `wait`, `inbox`, and `list`.

Acceptance:

- [ ] No duplicated source of truth between `modes.json`, docs, and Rust hardcoded mode lists.
- [ ] Unknown mode ids, missing skill refs, ungranted `skill://` reads, and future `skills.*`
      operations fail closed or degrade with explicit warnings.
- [x] A contributor can add a local mode + skill bundle without changing Rust code.
- [ ] Normal `cargo test` stays external-service-free and live OpenAI/Postgres tests remain gated.

## P3 goal

Ship the handoff + sub-agent actor baseline without weakening the catalog boundaries:

- Handoff remains a catalog mode, not a hardcoded side path.
- `oh-my-pi-handoff` remains the procedural brief skill for handoff behavior.
- `agents.*` lands as a capability-gated SDK namespace, discoverable through the host catalog.
- Agent output returns as digests, artifacts, and `agent://` / `history://` resources, not raw
  transcript dumps into parent context.
- Actor orchestration extends the existing loop; it does not fork a second product runtime.

## P3 acceptance gate

- [ ] `agents.run`, `agents.spawn`, `agents.parallel`, and `agents.msg` are registered in the host
      catalog with docs, schemas, examples, grants, and denial behavior.
- [ ] `agents` is only defined in sandbox sessions with the required grants; other modes still see it
      as `undefined` or fail closed.
- [ ] Handoff mode uses `modes.json` for mode state and `skill://oh-my-pi-handoff` for the brief.
- [ ] Fan-out to multiple workers returns only bounded digests to the parent context.
- [ ] Sibling agents can coordinate through explicit messages.
- [ ] A crashing child agent is isolated, restarted or degraded by supervision policy, and recorded in
      replayable events.
- [ ] `agent://` and `history://` resources resolve through the resource gateway with grants and
      bounded previews.
- [ ] Approval-gated effects inside child agents still route through the same approval broker and
      timeout/default-deny behavior.
- [ ] Normal `cargo test` remains external-service-free.

## P3.0 Scope lock

- [ ] Add `tm-agents` only after the actor lifecycle has concrete users: Handoff, parallel subtasks,
      and resource replay.
- [ ] Keep the existing agent loop as the single owner of message accumulation, tool-call execution,
      result shaping, and `EventSink` callbacks.
- [ ] Do not fork a second runtime loop for sub-agents.
- [ ] Preserve the native Deno Serious Engineer path for `fs.*`, `code.*`, `proc.*`, artifacts, and
      HTTP-routed manual approvals.
- [ ] Keep OMP ACP replaceable; P3 should not make it the permanent SDK model.

Acceptance:

- [ ] P3 can be reviewed as an additive orchestration layer over the current server, sandbox, host,
      resource, artifact, and approval contracts.
- [ ] No raw shell escape hatch, ambient network, direct filesystem reach, or chat-native tool sprawl
      is introduced.

## P3.1 Actor lifecycle

- [ ] Define actor ids, parent/child links, status, scope, mode, budget, started/completed timestamps,
      and cancellation state.
- [ ] Persist actor lifecycle events in the session event log for replay.
- [ ] Add cancellation and timeout handling that leaves a replayable terminal state.
- [ ] Add supervision defaults for crash, timeout, approval denial, and quota exhaustion.
- [ ] Keep actor transcript storage out of parent model context by default.

Acceptance:

- [ ] A parent session can spawn an actor, observe progress, cancel it, and replay the outcome after
      reconnect.
- [ ] Actor failure is visible but does not corrupt the parent session.

## P3.2 Agent SDK surface

- [ ] Add capability names and grant checks for `agents.*`.
- [ ] Implement `agents.run` for one-shot child work with bounded digest return.
- [ ] Implement `agents.spawn` for long-running child sessions.
- [ ] Implement `agents.parallel` for bounded fan-out with aggregate digest.
- [ ] Implement `agents.msg` for explicit sibling/parent messages.
- [ ] Update `docs/sdk/tm-runtime.d.ts` and `tools.docs` together.
- [ ] Add tests for granted, denied, unknown method, invalid args, timeout, and output-spill paths.

Acceptance:

- [ ] The model can discover and use `agents.*` from `tools.search/docs` without loading the entire
      SDK catalog into the system prompt.
- [ ] Denied or unavailable `agents.*` calls fail closed with typed host errors.

## P3.3 Resources and artifacts

- [ ] Register `agent://<id>` for child output artifact previews.
- [ ] Register `history://<id>` for read-only child transcript previews.
- [ ] Spill large child output to `artifact://` rather than parent context.
- [ ] Include provenance links from parent events to child resources.
- [ ] Add bounded preview/list behavior for mobile clients.

Acceptance:

- [ ] Parent turns contain summaries and handles, not large child transcripts.
- [ ] Resource reads enforce grants and return shaped `ResourceContent` envelopes.

## P3.4 Handoff mode

- [ ] Keep Handoff's mode profile, default scope, voice cap, declared capabilities, and active skills
      in `modes.json`.
- [ ] Use `oh-my-pi-handoff` for the handoff brief template and constraints.
- [ ] Route implementation-heavy delegation prompts to Handoff without mutating identity.
- [ ] Preserve Serious Engineer voice cap `off` for the handoff/coding boundary.
- [ ] Add tests that Handoff prompt composition includes `skill://oh-my-pi-handoff` and does not load
      `miku-voice`.

Acceptance:

- [ ] Handoff behavior changes can be made by editing the catalog assets, not by hardcoding prompt
      strings in the server.
- [ ] Handoff remains precise, replayable, and approval-bound.

## P3.5 Client and replay support

- [ ] Surface actor progress in SSE with compact mobile-friendly events.
- [ ] Preserve reconnect/resume through `Last-Event-ID`.
- [ ] Expose child resource links without forcing transcript expansion.
- [ ] Reuse the existing approval UI for child-agent permission requests.
- [ ] Add Flutter Web/PWA smoke coverage for actor progress, approval, resource open, and reconnect.

Acceptance:

- [ ] A phone/browser can watch a delegated task, resolve approvals, open child artifacts, and resume
      after disconnect.
- [ ] Clients do not gain direct write authority over agents, memory, skills, or files.

## Deferred catalog work

- [ ] Full `tm-memory`, pgvector, BM25/RRF, graph recall, dream queue, and skill proposal generation
      remain P4.
- [ ] `skills.*` import/version/reload, provenance, audit/replay, MCP import gates, and self-evolution
      tiers remain P7.
- [ ] `drive.*` remains P5.
- [ ] Android OS packaging waits until the P3/P5 server and resource surfaces stabilize.
- [ ] Secret broker work and production egress hardening remain scoped hardening milestones.
- [ ] Do not relax manual approval, unknown-scheme fail-closed behavior, argv-vector `proc.run`, or
      linked-folder grant boundaries.
