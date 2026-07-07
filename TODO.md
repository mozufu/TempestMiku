# TODO

Last aligned: **2026-07-06**.

Active milestone: **P4** (P3-plus actor/mailbox/supervision/provenance closeout is complete;
see deferred catalog work at the end for post-P3+ scope).

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
      `crates/tm-modes/assets/`.
- [x] Configured persona assets can override the bundle through `TM_MODES_PATH`.
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
- [x] P3-plus foundation slice: `MailboxRegistry` now has bounded per-actor inbox queues,
      `Agent::run` can drain actor inbox messages into the next model turn, `InvocationCtx` carries
      live actor ids, and `agents.send/wait/inbox/list/cancel` are registered SDK calls.

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
      `agents.run / spawn / parallel / msg`. The §23 full surface (`pipeline`, `broadcast`, `send`,
      `wait`, `inbox`, `list`) is documented as **P3 stretch / in-P3-plus** in P3.2 below.
      Implementing MVP first satisfies the "concrete users first" scope-lock rule; the stretch items
      extend only after the MVP acceptance gate passes.

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
      existing test coverage in `tm-modes` and `tm-sandbox`.
- [x] A contributor can add a local mode + skill bundle without changing Rust code.
- [x] Normal `cargo test` stays external-service-free and live OpenAI/Postgres tests remain gated.

## P3 goal

Ship the handoff + sub-agent actor baseline without weakening the catalog boundaries:

- Handoff remains a catalog mode, not a hardcoded side path.
- `oh-my-pi-handoff` remains the procedural brief skill for handoff behavior.
- `agents.run/spawn/parallel/msg` land as the first capability-gated SDK namespace, discoverable
  through the host catalog.
- Agent output returns as digests, artifacts, and `agent://` / `history://` resources, not raw
  transcript dumps into parent context.
- Actor orchestration extends the existing loop; it does not fork a second product runtime.

## P3 acceptance gate

- [x] `agents.run`, `agents.spawn`, `agents.parallel`, and `agents.msg` are registered in the host
      catalog with docs, schemas, examples, grants, and denial behavior.
      **Resolved:** All four HostFns registered with full docs/schemas/examples/grants. All four are
      live with `ChatActorExecutor`. **P3-plus update:** `agents.send`, `agents.wait`,
      `agents.inbox`, and `agents.list` are now registered with matching docs/schemas/examples/grants;
      `agents.msg` delivers to live inboxes for running actors and keeps the seeded continuation
      fallback for completed actors.
- [x] `agents` is only defined in sandbox sessions with the required grants; other modes still see it
      as `undefined` or fail closed.
      **Resolved:** `AGENTS_PRELUDE` injected in `install_prelude` only when `grants.names().any(|n| n.starts_with("agents."))`; ungranted sessions keep the `undefined` placeholder. Tests: `deno_agents_namespace_wired_when_granted`, `deno_agents_namespace_undefined_without_grant`.
- [x] Handoff mode uses `modes.json` for mode state and `skill://oh-my-pi-handoff` for the brief.
      **Resolved:** Mode profile, default scope, voice cap, capabilities, and active skills live in
      `modes.json`; no server-side hardcodes. System prompt is catalog-driven via
      `ModesConfig::build_system_prompt`. `CapabilityGrants::permits` now supports glob patterns
      so `"agents.*"` in a mode profile activates all `agents.*` sandbox grants. Capabilities flow
      through `CodingTurn.capabilities` → `run_native_turn` → `options.grants`. Tests: persona
      handoff profile assertions, `!contains("miku-voice")` for handoff turn, `agents.*` capability
      declared, glob-permits unit tests in `tm-host`.
- [x] Fan-out to multiple workers returns only bounded digests to the parent context.
      **Resolved:** `agents.parallel` and `agents.run` return `ActorDigest` JSON (`actorId`, `summary`,
      `artifactUri`, `historyUri`) — raw transcripts are never injected into parent context by design.
      `ChatActorExecutor` runs sub-agents with `NullSink`; full output goes nowhere unless the actor
      writes to an artifact URI. Test: `agents_parallel_runs_all_and_returns_ordered_digests`.
- [x] Sibling agents can coordinate through explicit messages.
      **P3-plus update:** live resident delivery now uses per-actor bounded inbox queues plus
      `Agent::run` inbox draining. `agents.send/broadcast/wait/inbox/list/cancel/pipeline` are live;
      restart supervision, wall-clock budgets, fail-fast sibling groups, `StatusChanged`, and
      parent-event provenance links are now covered by the P3-plus closeout.
- [x] A crashing child agent is isolated, restarted or degraded by supervision policy, and recorded in
      replayable events.
      **Resolved (P3-plus):** child failures are marked with structured `FailureReason`, persisted as
      `actor_failed`, followed by replayable `actor_supervision` decisions. Restartable failures use
      `one_for_one`, `one_for_all`, or `rest_for_one` policy; max-restart exhaustion escalates.
- [x] `agent://` and `history://` resources resolve through the resource gateway with grants and
      bounded previews.
      **Resolved:** `agent://<id>` returns actor record JSON (including `artifact_uri`/`history_uri`);
      `agent://` lists all actors; `history://<id>` serves real bounded transcripts (P3.3).
- [x] Approval-gated effects inside child agents still route through the same approval broker and
      timeout/default-deny behavior.
      **Resolved (P3-plus):** Child actor sandboxes now use the live `HttpApprovalPolicy` +
      `ApprovalBroker` when manual approvals are enabled. `RosterCodingEventSink` routes approval
      and approval-resolution events from child worker threads into the parent session's replayable
      SSE/event log; timeout/default-deny behavior still holds when no live policy is configured.
- [x] Normal `cargo test` remains external-service-free.

## P3.0 Scope lock

- [x] Add `tm-agents` only after the actor lifecycle has concrete users: Handoff, parallel subtasks,
      and resource replay.
      **Resolved:** `agents.run` is live end-to-end (`ChatActorExecutor` → `Agent::run`).
- [x] `tm-agents` module layout per §23.8: `actor` (identity, lifecycle, behavior binding), `mailbox`
      (async queue, addressing, delivery + receipts, broadcast, wait/inbox), `orchestrate`
      (`agents.*` constructors, DAG handles wired by reference, cancel token entry point), `supervise` (supervision tree,
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
      **P3-plus update:** child `ActorSpec` grants still carry only caller `agents.*` grants for
      mailbox coordination; linked-folder `fs.*` / `code.*` / `proc.*` grants come from the sandbox
      config and remain approval-bound.
- [x] Sub-agents start with no conversation history; everything needed is in the assignment + shared
      context (encapsulation by design).
- [x] Persist actor lifecycle events in the session event log for replay: spawns, messages, results,
      supervision decisions.
      **Resolved (P3.5):** `ActorLifecycleEvent` types defined and emitted (Spawned/Completed/Failed)
      from all three actor functions; `AppState::wire_lifecycle_sink` routes events through
      `store.append_event` + SSE broadcast. **P3-plus update:** `MessageSent` now emits for delivered
      live mailbox messages. `Cancelled` is emitted by `agents.cancel`; failure paths now emit
      replayable `Supervision` decisions after `Failed`; status transitions now emit replayable
      `actor_status` events.
- [x] Add cancellation and timeout handling that leaves a replayable terminal state.
      **Resolved:** Failure paths in `agents.run` call `mark_failed` with structured `FailureReason`;
      `actor_failed` events now reach the SSE log (P3.5). **P3-plus update:** `agents.cancel`
      trips per-actor cancellation tokens and emits replayable `actor_cancelled`; wall-clock budget
      timeouts now trip the same actor cancellation token and record `FailureReason::Timeout`.
- [x] Add supervision defaults for crash, timeout, approval denial, and quota exhaustion, with restart
      strategies: `one_for_one` (default — restart only the dead child), `one_for_all` (restart a
      coupled group), `rest_for_one` (restart the dead child and those started after it).
      **Resolved:** `FailureReason` and `RestartStrategy` types defined in `supervise.rs`.
      **P3-plus update:** supervisor state now computes and emits strategy decisions, and `Cancel`
      actions are applied to siblings. `Restart` actions now respawn actors from stored restart
      templates with fresh cancellation/inbox state.
- [x] Enforce a depth limit to bound recursion; cost accounting rolls up to the parent session.
      **Resolved:** `ChatActorExecutor` checks `spec.depth >= spec.budget.max_depth` and returns
      `DepthExceeded`. **P3-plus update:** real caller-id threading is now in `InvocationCtx` and
      Deno child sandbox options; parent depth propagation remains deferred.
- [x] Apply per-actor budgets: wall-clock, heap, and egress caps with conservative defaults.
      **Resolved:** `ActorBudget { wall_ms: 120_000, max_depth: 4 }` as defaults.
      **P3-plus update:** `wall_ms` is enforced around tracked actor executor calls
      (`run`/`spawn`/`parallel`/`pipeline`). Heap/egress caps remain future hardening work.
- [x] Keep actor transcript storage out of parent model context by default.
      **Resolved:** `ChatActorExecutor` uses `NullSink` — no transcript reaches the parent context.
      Only the bounded `summary` string is returned to the orchestrator.

Acceptance:

- [x] A parent session can spawn an actor, observe progress, cancel it, and replay the outcome after
      reconnect.
      **Resolved:** spawn, observe (`agent://` gateway), and replay (P3.5 lifecycle events in SSE)
      all work. `agents.cancel` now covers parent-driven child cancellation and replay.
- [x] Actor failure is visible but does not corrupt the parent session.
      **Resolved:** `agents.run` catches executor errors, marks actor failed with `FailureReason`,
      and returns `HostError::HostCall` — parent session is unaffected.
- [x] "Let it crash" — a failing child aborts only its subtree; siblings continue.
      **Resolved:** isolation holds — each actor runs in a dedicated thread, failures are caught and
      marked via `mark_failed`; no parent corruption. **P3-plus update:** supervision `Cancel`
      actions now trip sibling tokens, and restart actions respawn actors from stored templates with
      fresh cancellation/inbox state.

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
- [x] Implement `agents.parallel([{role, task}, …])` — one-wave fan-out with bounded pool and ordered
      digest results.
      **Resolved:** Validates all args before executor check; tracks all actors as Running; fans out via
      `tokio::spawn` per actor (all run concurrently); awaits handles in submission order so results[i]
      matches tasks[i]; first actor failure returns error, remaining detached tasks update roster via Arc.
      Tests: denied, not-impl, empty array, invalid args, 3-actor ordered digests.
- [x] Implement `agents.msg(handle, text, opts?)` — send to a spawned actor (request/reply or
      fire-and-forget).
      **Resolved (P3.2 one-shot model, upgraded in first P3-plus slice):** P3 shipped a one-shot
      compatibility model. Fire-and-forget now delivers to the live bounded inbox for running actors
      while preserving the bounded message log; `opts.await = true` waits for a live reply when the
      target is running, and falls back to the seeded-continuation path for already completed actors.
      Unknown handle → `null` (unreachable receipt, §23.9). Continuation actors remain ephemeral.
- [x] Update `docs/sdk/tm-runtime.d.ts` and `tools.docs` together; never one without the other.
      **Resolved:** `tm-runtime.d.ts`, `AgentsMsgFn`/new mailbox HostFn `ToolDocs`, and
      `23-agents-orchestration.md` are aligned for the live inbox surface.
- [x] Add tests for: granted, denied, unknown method, invalid args, timeout, output-spill, and
      unreachable-handle paths.
      **Resolved:** `agents_run_denied_without_grant`, `agents_run_executor_not_configured_returns_not_implemented`,
      `agents_run_invalid_args_returns_invalid_args_error`, `agents_run_with_executor_tracks_and_returns_digest`.

**§23 full surface — see `## P3-plus` for the closed mailbox, supervision, and protocol checklist.**

Acceptance:

- [x] The model can discover and use `agents.*` from `tools.search/docs` without loading the entire
      SDK catalog into the system prompt.
      **Resolved:** P3 HostFns and the first P3-plus mailbox HostFns have full docs, schemas,
      examples, and grants; discoverable via `tools.search("agents")` or `tools.docs("agents.run")`.
- [x] Denied or unavailable `agents.*` calls fail closed with typed host errors.
      **Resolved:** `check_grant!` macro returns `CapabilityDeniedError`; missing executor returns
      `NotImplementedError`; invalid args return `InvalidArgsError`. Covered by test matrix.

## P3.3 Resources and artifacts

- [x] Register `agent://<id>` for child output artifact previews.
      **Resolved:** `AgentResourceHandler` reads `ActorRecord` JSON for `agent://<id>` and lists
      all actors for `agent://`. Artifact content (full output) deferred below.
- [x] Register `history://<id>` for read-only child transcript previews.
      **Resolved:** `HistoryResourceHandler` serves real transcripts from `MailboxRegistry.transcripts`
      with 50-line default pagination, `"start-end"` selector, `has_more` flag, and 1 KB preview.
- [x] Spill large child output to `artifact://` rather than parent context; only bounded digests
      return to the orchestrator's model context.
      **Resolved:** `ChatActorExecutor::run_to_digest` uses `CollectingSink` to capture all events;
      opens `ArtifactStore` for the parent session after run; new child artifact → `artifact_uri`;
      transcript → `history://<actor>`. `ActorDigest.history_content`
      carries the raw transcript (skipped from JSON so it never reaches the model); orchestrator
      stores it via `MailboxRegistry.store_transcript`.
- [x] Include provenance links from parent events to child resources.
      **Resolved:** `ActorRecord` carries `artifact_uri` and `history_uri`; exposed via `agent://<id>`
      resource JSON. **P3-plus update:** `actor_completed` events with child resource URIs are
      followed by replayable `actor_resources_linked` events carrying `source_event_seq`,
      `artifact_uri`, and `history_uri`, so audit/replay can recover links from one parent event log.
- [x] Add bounded preview/list behavior for mobile clients.
      **Resolved:** `HistoryResourceHandler::list` filters to actors with `history_uri` set;
      `HistoryResourceHandler::read` returns selector-bounded content with `has_more` and `preview`.

Acceptance:

- [x] Parent turns contain summaries and handles, not large child transcripts.
      **Resolved:** `ActorDigest.history_content` is `#[serde(skip)]`; model only sees `summary`.
- [x] Resource reads enforce grants and return shaped `ResourceContent` envelopes.
      **Resolved:** `HistoryResourceHandler` requires `CAP_HISTORY_READ`; returns `ResourceContent`
      with all fields populated.

## P3.4 Handoff mode

- [x] Keep Handoff's mode profile, default scope, voice cap, declared capabilities, and active skills
      in `modes.json` (currently present; verify no server-side hardcode duplicates it).
      **Resolved:** All state lives in `modes.json`; persona loading is fully catalog-driven.
- [x] Use `oh-my-pi-handoff` for the handoff brief template and constraints.
      **Resolved:** `active_skills: ["oh-my-pi-handoff"]` in catalog; `build_system_prompt` injects
      `## skill://oh-my-pi-handoff` section from the bundled SKILL.md.
- [x] Route implementation-heavy delegation prompts to Handoff without mutating identity.
      **Resolved:** Router triggers `["handoff", "delegate"]` at priority 100 in modes.json.
- [x] Preserve Serious Engineer voice cap `off` for the handoff/coding boundary.
      **Resolved:** Both modes have `voice_cap: "off"`; no voice injection at the handoff boundary.
- [x] Add tests that Handoff prompt composition includes `skill://oh-my-pi-handoff` and does not load
      `miku-voice`.
      **Resolved:** `coding_turn_prompt_uses_active_persona_bundle` asserts both.
- [x] Capability `agents.*` is declared in `modes.json` for Handoff; verify the grant activates when
      the sandbox session uses the Handoff mode profile.
      **Resolved:** `CapabilityGrants::permits` now supports `.*` globs; `CodingTurn.capabilities`
      carries mode-profile capabilities; `run_native_turn` merges them into sandbox grants. Tests:
      `capability_grants_glob_match` in `tm-host`; persona handoff test asserts `has_capability("agents.run")`.

Acceptance:

- [x] Handoff behavior changes can be made by editing the catalog assets, not by hardcoding prompt
      strings in the server.
      **Resolved:** Confirmed — zero handoff-specific strings in server Rust code.
- [x] Handoff remains precise, replayable, and approval-bound.
      **Resolved:** Child actors under Handoff use live HTTP approvals when manual approval mode is
      configured, and otherwise fail closed through default-deny timeout behavior.

## P3.5 Client and replay support

- [x] Surface actor progress in SSE with compact mobile-friendly events.
      **Resolved:** `ActorLifecycleEvent` (Spawned/Completed/Failed) emitted from `agents.run`,
      `agents.spawn`, and `agents.parallel` via `MailboxRegistry::emit_lifecycle`. Hook installed
      at startup by `AppState::wire_lifecycle_sink`; writes to store + broadcasts on session SSE
      channel. `InvocationCtx` carries `session_id` wired from `DenoSandboxOptions.session_id`.
      **P3-plus update:** delivered `agents.msg` / `agents.send` messages now emit `MessageSent`
      lifecycle events through the same hook.
- [x] Preserve reconnect/resume through `Last-Event-ID`.
      **Resolved:** SSE endpoint already fully implemented in P1; actor lifecycle events flow
      through the same `append_event` + broadcast path and are replayable.
- [x] Expose child resource links without forcing transcript expansion.
      **Resolved (P3.3):** `ActorDigest.artifact_uri` / `history_uri` returned in digests; served
      via `artifact://` and `history://` resource handlers.
- [x] Reuse the existing approval UI for child-agent permission requests.
      **Resolved (P3-plus):** Child approval requests emit ordinary `approval` /
      `approval_resolved` SSE events with `scope.actorId`, so existing server routes and Flutter
      approval UI resolve them without a second approval path.
- [x] Add Flutter Web/PWA smoke coverage for actor progress, approval, resource open, and reconnect.
      **Resolved (P3-plus):** Flutter subscribes to actor lifecycle event types; widget smoke covers
      actor progress, approval resolution, child `artifact://` open, and reconnect via remembered
      `Last-Event-ID`.

Acceptance:

- [x] A phone/browser can watch a delegated task, resolve approvals, open child artifacts, and resume
      after disconnect.
- [x] Clients do not gain direct write authority over agents, memory, skills, or files.

## P3-plus

Items deferred from the P3 MVP. Ordering within each section is priority order at planning time;
order between sections is not meaningful. No item here implies the previous milestone is incomplete —
each was explicitly closed by deferral.

### Actor mailbox — live resident messaging

The §23 full messaging surface. The foundation is now in place: per-actor bounded inbox queues,
`Agent::run` inbox draining, caller actor-id threading, and the first live mailbox SDK calls.

- [x] Live MPSC delivery: per-actor bounded inbox queue, `Agent::run` inbox draining loop,
      and request/reply waits. Required before any other item in this section.
      **Resolved:** `MailboxRegistry` creates bounded `tokio::mpsc` inboxes per actor, keeps a
      compatibility message log, returns failed receipts with `unreachable` / `backpressured`
      reasons for unreachable/full inboxes, and `tm-core`
      exposes an `InboxDrain` hook used by `ChatActorExecutor`.
- [x] `agents.send(to, text, opts?)` — fire-and-forget or request/reply to a live sibling.
- [x] `agents.wait(from?, timeout)` — block until a matching message arrives in the inbox.
- [x] `agents.inbox()` — drain all pending messages without blocking.
- [x] `agents.list()` — roster: peer ids, statuses, unread count, last activity timestamp.
- [x] `agents.broadcast(text)` — deliver a message to all currently live children.
- [x] `agents.pipeline(items, ...stages)` — staged map with a barrier between stages.
      **Resolved:** `agents.pipeline` is registered and prelude-backed. It runs ordered actor waves
      stage-by-stage, waits at each barrier, and feeds compact digest references (`agent://`,
      `history://`, artifact URI, bounded summary) into the next stage without re-inlining
      transcripts. The namespace wrapper also supports per-stage task builder functions.
- [x] Real caller actor-id threading through `InvocationCtx` (top-level orchestrator calls still use
      synthetic `"Root"`; child sandboxes receive their real actor id).

### Supervision and recovery

- [x] Active restart behavior: `one_for_one`, `one_for_all`, `rest_for_one` strategies.
      **Resolved:** `supervise.rs` computes replayable supervision decisions for restartable
      crashes/timeouts, max-restart escalation, terminal cancellation, and strategy-specific
      cancel/restart target sets. `MailboxRegistry` tracks parent supervisor state, stores restart
      templates, emits replayable `actor_supervision` decisions after `actor_failed`, applies
      `Cancel` actions to sibling tokens, and respawns `Restart` actions with fresh cancellation
      and inbox state.
- [x] Wall-clock budget enforcement per actor.
      **Resolved:** all `agents.run` / `spawn` / `parallel` / `pipeline` executor calls now pass
      through a shared `ActorBudget.wall_ms` timeout wrapper. Timeout trips the actor cancellation
      token and maps to `FailureReason::Timeout` for replayable failure/supervision handling.
- [x] Cancel token: parent can cancel a running child and receive a replayable `Cancelled`
      terminal event in the SSE stream.
      **Resolved:** `agents.cancel(target)` is registered and prelude-backed. Direct parents
      mark child actor records `terminated` + `cancelled`, trip the shared token used by
      `ChatActorExecutor` and child Deno sandboxes, suppress late completion events, and emit
      one replayable `actor_cancelled` event. `agent://<id>` exposes the terminal state on reconnect.
- [x] Subtree cancel-on-sibling-failure for `agents.spawn` / `agents.parallel`
      ("let it crash" — a failing child aborts its whole sibling group, not just itself).
      **Resolved:** `agents.parallel` waves run under an isolated `one_for_all` / zero-restart
      supervision group, so a failing branch cancels sibling wave members without restarting the
      whole wave. `agents.spawn` accepts `opts.supervision.group`, letting spawned siblings share
      the same fail-fast supervision group.

### Child actor approvals

- [x] Wire child actors to the live `HttpApprovalPolicy` + `ApprovalBroker` so
      approval-gated ops (file overwrite, unsafe `proc.run`) inside sub-agents can be
      resolved by the user rather than auto-denied.
      **Resolved:** `ChatActorExecutor` now receives the parent session id in `ActorSpec`,
      constructs child sandboxes in the parent session artifact/event namespace, and uses
      `RosterCodingEventSink` to emit replayable `approval` / `approval_resolved` events from
      dedicated actor threads.

### Messaging protocol invariants

- [x] Enforce plain-prose-only messages at the `agents.msg` / `agents.send` boundary;
      reject control-payload blobs (`{"type":"done"}` and similar) with `InvalidArgsError`.
      **Resolved:** `agents.msg`, `agents.send`, and `agents.broadcast` now share a plain-prose
      validator that rejects empty messages and complete JSON object/array payloads before delivery.
- [x] DAG acyclicity check: detect and reject targeted live waits that would cause an actor to
      wait on itself or its own descendant (structural deadlock prevention, not just timeout).
      **Resolved:** `MailboxRegistry` exposes actor lineage checks. `agents.msg(..., {await:true})`,
      `agents.send(..., {await:true})`, and targeted `agents.wait(from)` reject descendant wait
      edges with `InvalidArgsError`; synthetic top-level `Root` waits remain allowed.
- [x] Bounded mailbox + backpressure: drop or back-pressure senders when the inbox is full;
      `agents.broadcast` targets only currently live peers.
      **Resolved:** actor inboxes are bounded `tokio::mpsc` queues; `try_send` drops full-inbox
      sends with `failed` / `reason: "backpressured"` receipts instead of silently succeeding,
      unreachable targets use `reason: "unreachable"`, and broadcast filters through the registry's
      direct-live-children helper with ordered failed receipts for backpressure.

### Provenance

- [x] Full parent-`SessionEvent` linking: `ActorRecord` carries `artifact_uri` /
      `history_uri`, and parent event rows link those resources back to the producing
      `actor_completed` `session_event` sequence. Needed for full audit/replay from a single
      event log query.
      **Resolved:** `AppState::wire_lifecycle_sink` appends a replayable
      `actor_resources_linked` event immediately after each resource-bearing `actor_completed`
      event. The link payload includes `actor_id`, `source_event_type`, `source_event_seq`,
      `artifact_uri`, and `history_uri`; server smoke tests assert the link points at the
      persisted `actor_completed` sequence.
- [x] `StatusChanged` lifecycle events wired to SSE (`MessageSent` is now emitted for delivered
      live mailbox messages; `Spawned` / `Completed` / `Failed` were already live).
      **Resolved:** actor success, failure, cancellation, and restart paths emit `actor_status`
      rows through the same lifecycle hook and SSE/event-log path.

### Client coverage

- [x] Flutter Web/PWA smoke tests for actor progress (SSE lifecycle events), approval
      resolution, child resource open (`artifact://` / `history://`), and reconnect via
      `Last-Event-ID`.
      **Resolved:** `tm-e2e::run_actor_smoke` covers the public HTTP/SSE flow; Flutter widget
      smoke covers actor event subscription, approval resolution, child resource preview, and
      reattach with the remembered event cursor.

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
