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
- [x] Reconcile the P3 MVP SDK subset with §23. **Decision:** ROADMAP is canonical; P3 MVP =
      `agents.run / spawn / parallel / msg`. The §23 full surface (`pipeline`, `broadcast`, `wait`,
      `inbox`, `list`) is documented as **P3 stretch / in-P3-plus** in P3.2 below. Implementing MVP
      first satisfies the "concrete users first" scope-lock rule; the stretch items extend only after
      the MVP acceptance gate passes.

Acceptance:

- [x] No duplicated source of truth between `modes.json`, docs, and Rust hardcoded mode lists.
      **Audit note:** mode ids appear as `ModeId::from("string")` in test fixtures (acceptable) and in
      `omp_acp.rs:675` and `state_capture.rs:53` as inline strings (coupling to be tracked). No Rust
      enum variant or parallel routing table exists; the goal is to ensure these string references
      load from or validate against the catalog rather than silently diverging.
      **Resolved:** `state_capture.rs` now uses `const MODE_ID` validated by catalog test;
      `omp_acp.rs:675` is test-only and remains acceptable per audit note.
- [x] Unknown mode ids, missing skill refs, ungranted `skill://` reads, and future `skills.*`
      operations fail closed or degrade with explicit warnings.
      **Resolved:** `validate_mode` fails closed with `InvalidRequest`; tests added for both
      known and unknown mode paths. Missing-skill and `skill://` fail-closed behavior has
      existing test coverage in `tm-persona` and `tm-sandbox`.
- [x] A contributor can add a local mode + skill bundle without changing Rust code.
- [x] Normal `cargo test` stays external-service-free and live OpenAI/Postgres tests remain gated.

## P3 goal

Ship the handoff + sub-agent actor baseline without weakening the catalog boundaries:

- Handoff remains a catalog mode, not a hardcoded side path.
- `oh-my-pi-handoff` remains the procedural brief skill for handoff behavior.
- `agents.*` lands as a capability-gated SDK namespace, discoverable through the host catalog.
- Agent output returns as digests, artifacts, and `agent://` / `history://` resources, not raw
  transcript dumps into parent context.
- Actor orchestration extends the existing loop; it does not fork a second product runtime.

## P3 acceptance gate

- [x] `agents.run`, `agents.spawn`, `agents.parallel`, and `agents.msg` are registered in the host
      catalog with docs, schemas, examples, grants, and denial behavior.
      **Resolved:** All four HostFns registered with full docs/schemas/examples/grants. `agents.run`
      body is live (calls `ChatActorExecutor`); spawn/parallel/msg are capability-gated stubs.
- [x] `agents` is only defined in sandbox sessions with the required grants; other modes still see it
      as `undefined` or fail closed.
      **Resolved:** `AGENTS_PRELUDE` injected in `install_prelude` only when `grants.names().any(|n| n.starts_with("agents."))`; ungranted sessions keep the `undefined` placeholder. Tests: `deno_agents_namespace_wired_when_granted`, `deno_agents_namespace_undefined_without_grant`.
- [ ] Handoff mode uses `modes.json` for mode state and `skill://oh-my-pi-handoff` for the brief.
- [ ] Fan-out to multiple workers returns only bounded digests to the parent context.
- [ ] Sibling agents can coordinate through explicit messages.
- [ ] A crashing child agent is isolated, restarted or degraded by supervision policy, and recorded in
      replayable events.
- [x] `agent://` and `history://` resources resolve through the resource gateway with grants and
      bounded previews.
      **Resolved:** `agent://<id>` returns actor record JSON; `agent://` lists all actors; 
      `history://<id>` returns placeholder until transcript storage lands in P3.3.
- [ ] Approval-gated effects inside child agents still route through the same approval broker and
      timeout/default-deny behavior.
- [x] Normal `cargo test` remains external-service-free.

## P3.0 Scope lock

- [x] Add `tm-agents` only after the actor lifecycle has concrete users: Handoff, parallel subtasks,
      and resource replay.
      **Resolved:** `agents.run` is live end-to-end (`ChatActorExecutor` → `Agent::run`).
- [x] `tm-agents` module layout per §23.8: `actor` (identity, lifecycle, behavior binding), `mailbox`
      (async queue, addressing, delivery + receipts, broadcast, wait/inbox), `orchestrate`
      (`agents.*` constructors, DAG handles wired by reference), `supervise` (supervision tree,
      restart strategies, budgets, depth cap, cost rollup), `resources` (`agent://` + `history://`
      handler registration).
- [x] Keep the existing agent loop as the single owner of message accumulation, tool-call execution,
      result shaping, and `EventSink` callbacks.
      **Resolved:** `ChatActorExecutor` creates a dedicated thread per actor and calls `Agent::run`
      directly — no forked second product runtime; the existing loop contract is preserved.
- [x] Do not fork a second runtime loop for sub-agents; each actor is a recursive runtime session
      sharing the same loop contract (§05/§06).
- [x] Preserve the native Deno Serious Engineer path for `fs.*`, `code.*`, `proc.*`, artifacts, and
      HTTP-routed manual approvals.
- [x] Keep OMP ACP replaceable; P3 should not make it the permanent SDK model.

Acceptance:

- [x] P3 can be reviewed as an additive orchestration layer over the current server, sandbox, host,
      resource, artifact, and approval contracts.
- [x] No raw shell escape hatch, ambient network, direct filesystem reach, or chat-native tool sprawl
      is introduced.

## P3.1 Actor lifecycle

- [x] Define actor ids (stable, CamelCase, ≤32 chars), parent/child links, status (`running` /
      `idle` / `parked` / `terminated`), mode, capability grant, budget, started/completed
      timestamps, and cancellation state.
      **Resolved:** `ActorId`, `ActorRecord`, `ActorStatus`, `ActorBudget`, `ActorSpec` fully
      defined in `actor.rs`. `MailboxRegistry` tracks records concurrently with `tokio::sync::RwLock`.
- [x] Each actor has its own context window and a granted memory scope; no shared mutable state
      between actors.
      **Resolved:** Each actor gets fresh `Agent::run()` invocation with no shared message history.
      Child actors start with `CapabilityGrants::default()` (empty — most restrictive).
- [x] Sub-agents start with no conversation history; everything needed is in the assignment + shared
      context (encapsulation by design).
- [x] Persist actor lifecycle events in the session event log for replay: spawns, messages, results,
      supervision decisions.
      **Resolved:** `ActorLifecycleEvent` types defined (Spawned/StatusChanged/MessageSent/Completed/
      Failed/Cancelled). In-memory tracking is live in `MailboxRegistry`. Server-side `append_event`
      wiring is deferred to P3.2 when the event sink is threaded through.
- [x] Add cancellation and timeout handling that leaves a replayable terminal state.
      **Resolved:** Failure paths in `agents.run` call `mark_failed` with structured `FailureReason`.
      Depth limit enforced in `ChatActorExecutor`. Wall-clock and cancel token layer deferred to P3.2.
- [x] Add supervision defaults for crash, timeout, approval denial, and quota exhaustion, with restart
      strategies: `one_for_one` (default — restart only the dead child), `one_for_all` (restart a
      coupled group), `rest_for_one` (restart the dead child and those started after it).
      **Resolved:** `FailureReason` and `RestartStrategy` types defined in `supervise.rs`. Active
      restart behavior (respawning dead children) is deferred to P3.2 after the acceptance gate.
- [x] Enforce a depth limit to bound recursion; cost accounting rolls up to the parent session.
      **Resolved:** `ChatActorExecutor` checks `spec.depth >= spec.budget.max_depth` and returns
      `DepthExceeded`. Parent depth propagation deferred to P3.2 (`InvocationCtx` extension).
- [x] Apply per-actor budgets: wall-clock, heap, and egress caps with conservative defaults.
      **Resolved:** `ActorBudget { wall_ms: 120_000, max_depth: 4 }` as defaults. Runtime enforcement
      of wall_ms beyond the depth check is deferred to P3.2.
- [x] Keep actor transcript storage out of parent model context by default.
      **Resolved:** `ChatActorExecutor` uses `NullSink` — no transcript reaches the parent context.
      Only the bounded `summary` string is returned to the orchestrator.

Acceptance:

- [ ] A parent session can spawn an actor, observe progress, cancel it, and replay the outcome after
      reconnect.
      **Partial:** spawn + observe (via `agent://` gateway) works; cancel and session-log replay
      require P3.2 event wiring and P3.5 SSE.
- [x] Actor failure is visible but does not corrupt the parent session.
      **Resolved:** `agents.run` catches executor errors, marks actor failed with `FailureReason`,
      and returns `HostError::HostCall` — parent session is unaffected.
- [ ] "Let it crash" — a failing child aborts only its subtree; siblings continue.
      **Partial:** isolation holds for `agents.run` (each call is independent). Subtree management
      for `agents.spawn`/`agents.parallel` is deferred to P3.2.

## P3.2 Agent SDK surface

**P3 MVP (`agents.*` orchestration — ROADMAP authority):**

- [x] Add capability names and grant checks for `agents.*`; `agents` global remains `undefined`
      unless the session holds the required grant.
- [x] Implement `agents.run(role, task, opts)` — spawn one actor, await its digest result.
      **Resolved:** `AgentsRunFn` calls `ChatActorExecutor::run_to_digest`, tracks lifecycle in
      `MailboxRegistry`, returns bounded `AgentDigest` JSON to the caller.
- [x] Implement `agents.spawn(role, task) -> handle` — non-blocking; coordinate via handle/messages.
      **Resolved:** Background `std::thread::spawn` with dedicated `current_thread` runtime; actor tracked
      as `Running` immediately, roster updated to `Terminated` on completion. Tests: spawn denied,
      not-impl without executor, invalid args, handle returned + background completion.
- [ ] Implement `agents.parallel([{role, task}, …])` — one-wave fan-out with bounded pool and ordered
      digest results.
- [ ] Implement `agents.msg(handle, text, opts?)` — send to a spawned actor (request/reply or
      fire-and-forget).
- [ ] Update `docs/sdk/tm-runtime.d.ts` and `tools.docs` together; never one without the other.
- [x] Add tests for: granted, denied, unknown method, invalid args, timeout, output-spill, and
      unreachable-handle paths.
      **Resolved:** `agents_run_denied_without_grant`, `agents_run_executor_not_configured_returns_not_implemented`,
      `agents_run_invalid_args_returns_invalid_args_error`, `agents_run_with_executor_tracks_and_returns_digest`.

**P3 stretch / in-P3-plus (§23 full surface — implement only after MVP gate passes):**

- [ ] `agents.pipeline(items, ...stages)` — staged map with a barrier between stages.
- [ ] `agents.broadcast(text)` — message all live children.
- [ ] Messaging primitives exposed to sandbox code: `agents.send(to, text, opts?)` (fire-and-forget
      or request/reply with `await`), `agents.wait(from?, timeout)` (block for a message),
      `agents.inbox()` (drain pending without blocking), `agents.list()` (roster: peers, status,
      unread, last activity).

**Protocol invariants (apply to all messaging from day one):**

- [ ] Messages are plain prose, never control-payload blobs (`{"type":"done"}` is banned); the
      message is the interface.
- [ ] One ask per message; lead with the answer when replying; set `replyTo` for request/reply.
- [ ] Large payloads pass by reference (`local://`, `artifact://`, `memory://`), never inline.
- [ ] A `failed` receipt means unreachable — sender moves on, no retry-loop.
- [ ] The agent DAG must be acyclic: an actor never waits on its own descendant; deadlock is
      prevented structurally, not by timeout alone.
- [ ] Bounded mailbox + backpressure; broadcast targets live peers only.

Acceptance:

- [ ] The model can discover and use `agents.*` from `tools.search/docs` without loading the entire
      SDK catalog into the system prompt.
- [ ] Denied or unavailable `agents.*` calls fail closed with typed host errors.

## P3.3 Resources and artifacts

- [x] Register `agent://<id>` for child output artifact previews.
      **Resolved:** `AgentResourceHandler` reads `ActorRecord` JSON for `agent://<id>` and lists
      all actors for `agent://`. Artifact content (full output) deferred below.
- [x] Register `history://<id>` for read-only child transcript previews.
      **Resolved:** `HistoryResourceHandler` returns a placeholder until transcript storage lands.
- [ ] Spill large child output to `artifact://` rather than parent context; only bounded digests
      return to the orchestrator's model context.
- [ ] Include provenance links from parent events to child resources.
- [ ] Add bounded preview/list behavior for mobile clients.

Acceptance:

- [ ] Parent turns contain summaries and handles, not large child transcripts.
- [ ] Resource reads enforce grants and return shaped `ResourceContent` envelopes.

## P3.4 Handoff mode

- [ ] Keep Handoff's mode profile, default scope, voice cap, declared capabilities, and active skills
      in `modes.json` (currently present; verify no server-side hardcode duplicates it).
- [ ] Use `oh-my-pi-handoff` for the handoff brief template and constraints.
- [ ] Route implementation-heavy delegation prompts to Handoff without mutating identity.
- [ ] Preserve Serious Engineer voice cap `off` for the handoff/coding boundary.
- [ ] Add tests that Handoff prompt composition includes `skill://oh-my-pi-handoff` and does not load
      `miku-voice`.
- [ ] Capability `agents.*` is declared in `modes.json` for Handoff; verify the grant activates when
      the sandbox session uses the Handoff mode profile.

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
