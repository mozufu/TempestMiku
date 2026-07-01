# TODO

Active milestone: **P2 - personal-assistant baseline**.

`ROADMAP.md` remains the canonical milestone order. This file is the working execution checklist for
P2 and should stay aligned with product docs §21, §22, §27, and §29.

P1 project manager + remote control is complete. P2 is the point where TempestMiku becomes a fuller
personal companion again without weakening the runtime bet: one `execute` loop, streaming-first
events, capability grants, and manual approvals remain the boundaries.

## P2 goal

Ship the personal-assistant baseline:

- Miku identity is constant and loaded through the persona layer.
- Voice intensity floats by mode and seriousness; Serious Engineer and Handoff stay capped at `關`.
- Profile/user recall is injected into turns with bounded context.
- Personal-assistant state capture proposes durable memories instead of silently writing them.
- Negative-state grounding stabilizes first and suggests one action that fits in 10 minutes or less.
- Proactivity is useful but bounded: local planning and reminders are allowed, external/destructive or
  sensitive writes require explicit approval.

## P2 acceptance gate

- [ ] Token-by-token responses over SSE include the active persona/mode/memory context path.
- [ ] Personal Assistant mode can recall profile facts and scoped recall chunks across sessions.
- [ ] Durable memory writes are approval-gated: approve writes, deny does not write, timeout defaults
      to deny, and all outcomes are replayable as events.
- [ ] The client can show and resolve memory write proposals through the same approval flow.
- [ ] Negative-State Grounding routes correctly, keeps health over productivity, and ends with no more
      than one <=10-minute next action.
- [ ] Serious Engineer mode still uses voice cap `關` and does not inherit high-cuteness output from
      the personal-assistant layer.
- [ ] Normal `cargo test` remains external-service-free.
- [ ] Gated Postgres coverage proves P2 memory rows, write proposals, approval outcomes, and replay.

## P2.0 Scope lock

- [ ] Treat P2 as an extension of existing `tm-persona`, `tm-server::memory`, `session_events`, and
      approval broker surfaces.
- [ ] Do not extract a full `tm-memory` crate until there is a second concrete user for the abstraction.
- [ ] Keep profile facts + recall chunks as the P2/P1 minimum store; defer vector, BM25/RRF, graph,
      dreaming, and skill generation to P4.
- [ ] Preserve the P0/P1 coding backend boundary: OMP ACP is replaceable, native Deno remains the
      dogfood path for `fs.*`, `code.*`, `proc.*`, artifacts, and HTTP-routed approvals.

Acceptance:

- [ ] P2 implementation can be reviewed as additive product behavior over the existing server loop.
- [ ] No new runtime loop, raw shell escape hatch, or chat-native tool sprawl is introduced.

## P2.1 Persona asset and prompt overlay

- [ ] Add a first-class persona prompt builder that composes:
      `SOUL identity + active mode addendum + voice overlay + bounded memory context`.
- [ ] Load configurable persona assets from a `TM_PERSONA_ASSET_PATH` or config-equivalent path.
- [ ] Preserve degraded boot behavior when assets are missing, with a visible warning in session state.
- [ ] Represent the voice overlay separately from mode addenda so seriousness can lower voice intensity
      without changing identity.
- [ ] Add tests for loaded assets, degraded assets, prompt composition order, and Serious Engineer
      voice cap.

Acceptance:

- [ ] Personal Assistant prompts contain identity, mode, voice, and memory sections in stable order.
- [ ] Serious Engineer prompts contain identity but keep voice cap `關`.
- [ ] Missing persona assets do not prevent boot or session creation.

## P2.2 Personal Assistant mode baseline

- [ ] Make Personal Assistant the default conversational mode for non-coding planning, writing,
      reminders, decisions, and open-loop cleanup.
- [ ] Ensure Personal Assistant mode only receives conversational/light memory capabilities in its
      addendum, not engineering host capabilities.
- [ ] Add router tests for default personal-assistant prompts, explicit user lock, and override back to
      Serious Engineer.
- [ ] Keep mode badges and `ModeChanged` replay behavior from P1.

Acceptance:

- [ ] A normal planning/reminder prompt routes to Personal Assistant and emits mode state if needed.
- [ ] A code/safety/irreversible prompt routes or locks to Serious Engineer with voice cap `關`.
- [ ] User locks remain final.

## P2.3 Memory auto-context and profile recall

- [ ] Expand `MemoryContext` beyond raw lists into a bounded prompt block with:
      profile facts, scoped recall chunks, provenance labels, and budget metadata.
- [ ] Keep the auto-injected memory budget around the §22 target of ~1600 tokens.
- [ ] Inject profile facts for Brian (`subject = brian`) and scoped recall for the active mode scope.
- [ ] Add a minimal ToM/profile synthesis hook with an <=800 character cap and a configurable cadence;
      keep it off for Serious Engineer turns unless explicitly useful.
- [ ] Add tests for budget trimming, provenance rendering, empty-memory degradation, and cross-session
      profile recall.

Acceptance:

- [ ] P2 turns include useful memory when present and a small/no-op block when absent.
- [ ] Memory context cannot swamp the user prompt or large tool outputs.
- [ ] Serious Engineer remains precise and does not over-personalize technical replies.

## P2.4 Memory write proposals and approvals

- [ ] Define a P2 memory write proposal shape for durable notes/facts:
      subject, predicate/object or note text, confidence, scope, source session/event, reason, and
      redaction status.
- [ ] Emit `write_proposal` events for memory writes before any durable assertion is stored.
- [ ] Reuse the existing approval broker and POST approval resolution path for approve/deny/timeout.
- [ ] On approve, write to `profile_facts` or `recall_chunks` with provenance.
- [ ] On deny or timeout, write nothing durable and append a replayable resolution event.
- [ ] Add redaction before disk for obvious secrets/tokens.
- [ ] Add tests for approve, deny, timeout/default-deny, stale proposal, idempotency, and replay.

Acceptance:

- [ ] Miku can propose "remember this" without silently changing memory.
- [ ] Every durable P2 memory assertion has provenance.
- [ ] Timeout defaults to deny and is observable.

## P2.5 Personal-assistant state capture

- [ ] Implement the `personal-assistant-state-capture` behavior as proposal logic, not direct writes.
- [ ] Capture stable preferences, active projects/open loops, commitments/deadlines, decisions,
      recurring blind spots, shipped artifacts, and reusable workflows.
- [ ] Explicitly avoid passing moods, one-off complaints, secrets, raw logs, large notes, and sensitive
      PII unless the user asks.
- [ ] Keep project-specific commands in project docs or project memory, not global user memory.
- [ ] Add tests for "capture" and "do not capture" examples.

Acceptance:

- [ ] Stable user/project signal produces one-line memory proposals.
- [ ] Sensitive or transient content does not produce durable proposals by default.
- [ ] Proposed memories are concise enough to approve from the client.

## P2.6 Negative-State Grounding

- [ ] Preserve mode 3 as a conversational posture, not a new capability set.
- [ ] Add router triggers for overwhelmed, exhausted, self-deprecating, spiraling, and stuck language.
- [ ] Prompt behavior must stabilize first, reduce demand, avoid shame, and choose one <=10-minute
      next action only after grounding.
- [ ] Do not create memory writes from negative-state content unless the user explicitly asks.
- [ ] Add tests for routing, response constraints, health-over-productivity, and no unsolicited memory
      write proposal.

Acceptance:

- [ ] A negative-state prompt routes to mode 3 and never becomes a productivity lecture.
- [ ] The response proposes at most one tiny next step.
- [ ] Health override beats shipping pressure.

## P2.7 Bounded proactivity and reminders

- [ ] Represent local reminder/open-loop suggestions as proposals or project items, not external
      commitments.
- [ ] Allow safe local actions: summarize, draft, organize TODOs, surface open loops, and suggest next
      steps.
- [ ] Gate external, destructive, spend, publish/send, sensitive-memory, and drive-link actions through
      approval.
- [ ] Keep scheduled/cron behavior deferred unless it can reuse the existing session event stream and
      deny-by-default bounds.
- [ ] Add tests for safe proactivity, gated proactivity, and cron-mode deny/defer behavior where touched.

Acceptance:

- [ ] Miku can be usefully proactive inside the app without acting outside Brian's approval boundary.
- [ ] Any external commitment remains impossible without explicit approval.

## P2.8 Client support

- [ ] Render `write_proposal` events in Flutter Web/PWA.
- [ ] Let the user approve/deny memory proposals through the existing approval resolution flow.
- [ ] Show memory proposal text, scope, and provenance in a compact mobile-friendly layout.
- [ ] Preserve reconnect/resume behavior for pending proposals and resolved outcomes.
- [ ] Add widget/smoke coverage for memory proposal display, approve, deny, and reconnect.

Acceptance:

- [ ] A phone/browser can review a proposed memory and approve or deny it.
- [ ] Pending proposals survive disconnect/reconnect.
- [ ] Client UI does not gain any direct memory-write authority.

## P2.9 Persistence and resource surface

- [ ] Keep normal tests on the in-memory store.
- [ ] Extend gated Postgres tests for P2 memory proposal state and approved durable rows.
- [ ] Register or document minimal `memory://` resources needed for P2:
      `memory://root`, `memory://user-model`, and scoped recall previews.
- [ ] Ensure unknown memory schemes and missing grants fail closed through the resource gateway.
- [ ] Keep large memory/debug output out of model context; use bounded previews or artifacts.

Acceptance:

- [ ] `cargo test` does not require Postgres, network-only services, or live OpenAI calls.
- [ ] `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=... cargo test -p tm-server` covers replay,
      proposals, approvals, profile facts, and recall chunks.
- [ ] `memory://` access is bounded and policy-checked.

## P2.10 Docs and parity fixtures

- [ ] Update product docs if implementation changes any behavior described in §21, §22, §27, or §29.
- [ ] Add small parity fixtures for SOUL/miku-voice/personal-assistant-state-capture behavior if the
      real deployment assets are not checked into this repository.
- [ ] Document config/env required for persona assets and memory approval behavior.
- [ ] Keep roadmap wording accurate: do not mark P2 done until the acceptance gate above passes.

Acceptance:

- [ ] Design docs and implementation agree.
- [ ] A new contributor can tell which P2 behavior is implemented, deferred, or gated.

## P2 suggested implementation order

1. [ ] Persona prompt builder and tests.
2. [ ] Memory context shape, budget, and profile recall tests.
3. [ ] Memory write proposal data model and server event path.
4. [ ] Approval-backed write commit path for profile facts and recall chunks.
5. [ ] Personal-assistant state-capture proposal rules.
6. [ ] Negative-State Grounding routing and response constraints.
7. [ ] Flutter Web/PWA write proposal UI and reconnect coverage.
8. [ ] Gated Postgres coverage and docs sweep.

## P2 non-goals

- [ ] Do not implement full `tm-memory`, pgvector, BM25/RRF, graph recall, or dreaming; these belong to
      P4 unless a P2 task proves a smaller extraction is needed.
- [ ] Do not implement `agents.*`; this belongs to P3.
- [ ] Do not implement `drive.*`; this belongs to P5.
- [ ] Do not implement Android OS packaging before P2/P3 product surfaces stabilize.
- [ ] Do not implement self-evolution writes, skill imports, MCP reload gates, or secret broker work;
      these belong to P7 or later scoped milestones.
- [ ] Do not relax manual approval, unknown-scheme fail-closed behavior, argv-vector `proc.run`, or
      linked-folder grant boundaries.
