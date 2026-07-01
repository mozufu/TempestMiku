# TODO

This file archives the completed P1 execution checklist. `ROADMAP.md` remains the canonical active
milestone order and source of deferred namespace placement.

## P1 — project manager + remote control

Status: complete. Current active work has moved to the P2 personal-assistant baseline in
`ROADMAP.md`.

P1 is done only when the server/client surface preserves the current TempestMiku parity constraints:
Miku identity remains constant, Serious Engineer voice cap stays `關`, manual approvals remain the
choke point for gated actions, and reconnect/replay does not lose project state or streaming events.

### P1.1 Mode router

- [x] Add session-level active mode state: current mode, router reason, lock/override source, and
      updated timestamp.
- [x] Implement mode suggest/apply/lock/unlock/override server API.
- [x] Emit `ModeChanged` events through the session event log and SSE stream.
- [x] Replay `ModeChanged` correctly from `Last-Event-ID`.
- [x] Keep Serious Engineer voice cap at `關`; lock/override must not weaken identity or voice rules.
- [x] Add tests for router fires, user lock wins, unlock returns to router behavior, and event replay.

Acceptance:

- [x] A normal coding prompt routes to Serious Engineer and emits a visible `ModeChanged`.
- [x] A user lock prevents router changes until unlock.
- [x] Reconnecting with `Last-Event-ID` replays mode state without duplicate/conflicting badges.
- [x] Serious Engineer mode always resolves to voice cap `關`.

### P1.2 Project state views

- [x] Define minimal data model for project status, open loops, decisions, and next actions.
- [x] Build project-manager views from session events and minimal memory summary.
- [x] Add read endpoints for project overview, open loops, decisions, and next actions.
- [x] Persist enough state for reconnect and cross-session continuity.
- [x] Add tests for status generation, decision/open-loop persistence, and reconnect behavior.

Acceptance:

- [x] A coding session can produce a project overview with current status and next actions.
- [x] Decisions and open loops survive reconnect and can be listed independently.
- [x] Project views preserve provenance back to session events or memory summary.

### P1.3 Session promotion

- [x] Implement `POST /sessions/:id/promote`.
- [x] Promote selected summary, open loops, decisions, `artifact://`, `linked://`, and
      `workspace://session/*` refs into `project://` resources.
- [x] Store provenance for every promoted item: source session, event id, artifact/resource URI, and
      selected timestamp.
- [x] Treat user-initiated promotion as approval.
- [x] Route Miku-initiated promotion through approval before writing project state.
- [x] Add tests for promotion, idempotency, provenance, and approval-required proposal flow.

Acceptance:

- [x] `POST /sessions/:id/promote` returns a `project://<id>` URI and all promoted targets.
- [x] Running promotion twice with the same selection does not duplicate project items.
- [x] Every promoted item can be traced back to its source session/event/resource.
- [x] Miku-proposed promotion denies by default on timeout.

### P1.4 Session resource gateway

- [x] Add session-scoped `read`, `preview`, and `list` gateway endpoints.
- [x] Support `artifact://`, `linked://`, `workspace://session/*`, and `project://` resources.
- [x] Keep unknown schemes, missing grants, and path traversal fail-closed.
- [x] Return bounded previews and spill/handle large content through artifacts where needed.
- [x] Add tests for each supported scheme, denial path, unknown scheme, and stale/missing resource.

Acceptance:

- [x] Client-visible resource links from session/project views can be opened through the gateway.
- [x] Unknown or ungranted schemes return policy errors, not raw host failures.
- [x] Large resources return bounded previews with stable resource/artifact handles.

### P1.5 Flutter Web/PWA remote control

- [x] Show mode badge, streamed response, final response, and session status.
- [x] Send messages over the existing session API.
- [x] Subscribe to SSE and resume via `Last-Event-ID`.
- [x] Render approval prompts and support approve/deny.
- [x] Open resource links through the session resource gateway.
- [x] Add mobile-sized smoke coverage for chat, streaming, approval, resource link, disconnect, and
      resume.

Acceptance:

- [x] A phone/browser can attach to an active session, send a task, and watch streaming events.
- [x] Approval prompts can be resolved from the client and are reflected in the server event stream.
- [x] Disconnect/reconnect resumes without losing final response, mode badge, or pending approvals.

### P1.6 Persistence boundary hardening

- [x] Keep normal `cargo test` external-service-free.
- [x] Add gated Postgres coverage for event replay.
- [x] Add gated Postgres coverage for memory rows used by P1 views.
- [x] Add gated Postgres coverage for approvals.
- [x] Add gated Postgres coverage for artifact/resource references.
- [x] Document the env gate so CI/local defaults do not require Postgres.

Acceptance:

- [x] `cargo test` passes without Postgres or network-only services.
- [x] Gated Postgres tests run only when the documented env var is set.
- [x] Event replay, approval state, and project/resource references have persistence coverage.

## P1 non-goals

- [x] Do not implement `agents.*`; this belongs to P3.
- [x] Do not implement full `memory.*` or dreaming; these belong to P2/P4.
- [x] Do not implement `drive.*`; this belongs to P5.
- [x] Do not implement `secrets.use` or full `http.*` egress hardening; these belong to P7 or P5
      research if live egress becomes necessary.
- [x] Do not start Android OS packaging before the P2/P3 product surfaces stabilize.
