# TODO

This file tracks the active P1 execution checklist. `ROADMAP.md` remains the canonical milestone
order and source of deferred namespace placement.

## P1 — project manager + remote control

P1 is done only when the server/client surface preserves the current TempestMiku parity constraints:
Miku identity remains constant, Serious Engineer voice cap stays `關`, manual approvals remain the
choke point for gated actions, and reconnect/replay does not lose project state or streaming events.

### P1.1 Mode router

- [ ] Add session-level active mode state: current mode, router reason, lock/override source, and
      updated timestamp.
- [ ] Implement mode suggest/apply/lock/unlock/override server API.
- [ ] Emit `ModeChanged` events through the session event log and SSE stream.
- [ ] Replay `ModeChanged` correctly from `Last-Event-ID`.
- [ ] Keep Serious Engineer voice cap at `關`; lock/override must not weaken identity or voice rules.
- [ ] Add tests for router fires, user lock wins, unlock returns to router behavior, and event replay.

Acceptance:

- [ ] A normal coding prompt routes to Serious Engineer and emits a visible `ModeChanged`.
- [ ] A user lock prevents router changes until unlock.
- [ ] Reconnecting with `Last-Event-ID` replays mode state without duplicate/conflicting badges.
- [ ] Serious Engineer mode always resolves to voice cap `關`.

### P1.2 Project state views

- [ ] Define minimal data model for project status, open loops, decisions, and next actions.
- [ ] Build project-manager views from session events and minimal memory summary.
- [ ] Add read endpoints for project overview, open loops, decisions, and next actions.
- [ ] Persist enough state for reconnect and cross-session continuity.
- [ ] Add tests for status generation, decision/open-loop persistence, and reconnect behavior.

Acceptance:

- [ ] A coding session can produce a project overview with current status and next actions.
- [ ] Decisions and open loops survive reconnect and can be listed independently.
- [ ] Project views preserve provenance back to session events or memory summary.

### P1.3 Session promotion

- [ ] Implement `POST /sessions/:id/promote`.
- [ ] Promote selected summary, open loops, decisions, `artifact://`, `linked://`, and
      `workspace://session/*` refs into `project://` resources.
- [ ] Store provenance for every promoted item: source session, event id, artifact/resource URI, and
      selected timestamp.
- [ ] Treat user-initiated promotion as approval.
- [ ] Route Miku-initiated promotion through approval before writing project state.
- [ ] Add tests for promotion, idempotency, provenance, and approval-required proposal flow.

Acceptance:

- [ ] `POST /sessions/:id/promote` returns a `project://<id>` URI and all promoted targets.
- [ ] Running promotion twice with the same selection does not duplicate project items.
- [ ] Every promoted item can be traced back to its source session/event/resource.
- [ ] Miku-proposed promotion denies by default on timeout.

### P1.4 Session resource gateway

- [ ] Add session-scoped `read`, `preview`, and `list` gateway endpoints.
- [ ] Support `artifact://`, `linked://`, `workspace://session/*`, and `project://` resources.
- [ ] Keep unknown schemes, missing grants, and path traversal fail-closed.
- [ ] Return bounded previews and spill/handle large content through artifacts where needed.
- [ ] Add tests for each supported scheme, denial path, unknown scheme, and stale/missing resource.

Acceptance:

- [ ] Client-visible resource links from session/project views can be opened through the gateway.
- [ ] Unknown or ungranted schemes return policy errors, not raw host failures.
- [ ] Large resources return bounded previews with stable resource/artifact handles.

### P1.5 Flutter Web/PWA remote control

- [ ] Show mode badge, streamed response, final response, and session status.
- [ ] Send messages over the existing session API.
- [ ] Subscribe to SSE and resume via `Last-Event-ID`.
- [ ] Render approval prompts and support approve/deny.
- [ ] Open resource links through the session resource gateway.
- [ ] Add mobile-sized smoke coverage for chat, streaming, approval, resource link, disconnect, and
      resume.

Acceptance:

- [ ] A phone/browser can attach to an active session, send a task, and watch streaming events.
- [ ] Approval prompts can be resolved from the client and are reflected in the server event stream.
- [ ] Disconnect/reconnect resumes without losing final response, mode badge, or pending approvals.

### P1.6 Persistence boundary hardening

- [ ] Keep normal `cargo test` external-service-free.
- [ ] Add gated Postgres coverage for event replay.
- [ ] Add gated Postgres coverage for memory rows used by P1 views.
- [ ] Add gated Postgres coverage for approvals.
- [ ] Add gated Postgres coverage for artifact/resource references.
- [ ] Document the env gate so CI/local defaults do not require Postgres.

Acceptance:

- [ ] `cargo test` passes without Postgres or network-only services.
- [ ] Gated Postgres tests run only when the documented env var is set.
- [ ] Event replay, approval state, and project/resource references have persistence coverage.

## P1 non-goals

- [ ] Do not implement `agents.*`; this belongs to P3.
- [ ] Do not implement full `memory.*` or dreaming; these belong to P2/P4.
- [ ] Do not implement `drive.*`; this belongs to P5.
- [ ] Do not implement `secrets.use` or full `http.*` egress hardening; these belong to P7 or P5
      research if live egress becomes necessary.
- [ ] Do not start Android OS packaging before the P1 server API stabilizes.
