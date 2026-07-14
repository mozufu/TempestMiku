# TODO

Last aligned: **2026-07-14**.

Active milestone: **P6.5 — review-first Android quick capture**.

`ROADMAP.md` is the milestone-order and completion-history source. The approved long-range route is
captured in `plans/one-shot-future-roadmap.md`. This file contains only the active executable slice,
its regression boundaries, and the queued order.

## P6.5 Outcome

An Android owner can open one bounded quick-capture draft from a launcher shortcut or Quick Settings
tile, edit it, and explicitly choose the current or a new session before sending. Receiving the
native intent never sends, pairs, approves, grants authority, or runs a model.

## P6.5 Contract

- [ ] Define one versioned native quick-capture intent with no URI grants, attachments, external
      origins, or ambient authority.
- [ ] Accept only sanitized bounded text and keep at most one cold-start payload; empty input opens
      an empty editable draft instead of sending.
- [ ] Route warm and cold intents through the existing platform event bridge into one Flutter review
      sheet with editable text and explicit current/new-session choices.
- [ ] Add a launcher shortcut and Quick Settings tile that both reuse that single intent contract.
- [ ] Keep widget support out unless it can reuse the same bounded intent and review sheet without a
      new send path or background authority.
- [ ] Reuse the authenticated durable message API, stable client ids, session creation, event replay,
      and existing composer semantics only after the owner presses Send.
- [ ] Preserve process-death recovery without duplicate previews, messages, sessions, or turns.
- [ ] Keep native Android free of model, sandbox, agent-loop, pairing, approval, filesystem, and
      arbitrary-network execution.

## Deterministic Verification

- [ ] Pure Kotlin tests cover intent/action matching, sanitization and bounds, empty input, URI or
      attachment rejection, and single pending cold-start delivery.
- [ ] Flutter parser/widget tests cover editable review, cancel/no-send, current/new-session routing,
      warm replacement, cold-start recovery, and no duplicate preview or send.
- [ ] Flutter analyze, the full Flutter suite, focused Android unit tests, and affected Rust/Web gates
      pass.
- [ ] Merged-manifest and APK inspection prove only the intended shortcut/tile components are
      exported and that release ABI/signing, HTTPS-only networking, and backup exclusion remain
      intact.

## Physical And Live Closeout

- [ ] Build an arm64 release APK with the established signing certificate and install it in place on
      the paired Android 15 device without losing credentials or session state.
- [ ] Prove launcher-shortcut and Quick Settings entry from foreground, background, and killed states.
- [ ] Prove empty/edit/cancel paths send nothing and distinct current/new-session confirmations each
      create exactly one durable user message and turn.
- [ ] Prove process death and repeated intent delivery do not replay a draft or duplicate the message,
      session, or turn.
- [ ] Record replayable evidence and align product §27, `ROADMAP.md`, `TODO.md`, and `AGENTS.md`
      before marking P6.5 complete.

## Regression Invariants

- Preserve pairing, secure storage, HTTPS-only release networking, backup exclusion, release signing,
  stable production push encryption, UnifiedPush/ntfy, actionable notifications, and Firebase/FCM
  absence.
- Preserve review-before-send for Sharesheet and selected-text imports; quick capture joins that
  explicit review model and does not weaken notification Send semantics.
- Keep captured text untrusted and authority-free. It grants no capability, resource, URI, pairing,
  approval, filesystem, or network authority.
- Reuse the durable turn queue, event log, SSE replay, and server-owned session authority.
- Keep OMP ACP replaceable and native Deno as the coding dogfood path.

## Queued After P6.5

1. P6.6 self-hosted voice capture and formal P6 closeout.
2. P8 fuller memory with self-hosted Postgres FTS + pgvector hybrid recall.
3. P7.2b Auto-mode persona proposals with durable manual activation and rollback.
4. P9 production egress and opaque secret broker.
5. P10 MCP catalog and live research through the P9 boundary.

## Demand-Triggered Or Permanently Out

- Demand-triggered: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and additional sandbox
  backends.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.
