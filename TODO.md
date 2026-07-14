# TODO

Last aligned: **2026-07-14**.

Active milestone: **P6.4 — actionable Android notifications**.

`ROADMAP.md` is the milestone-order and completion-history source. The approved long-range route is
captured in `plans/one-shot-future-roadmap.md`. This file contains only the active executable slice,
its regression boundaries, and the queued order.

## P6.4 Outcome

An Android owner can open the exact notification context or explicitly send a bounded inline reply
while Flutter is foregrounded, backgrounded, or killed. The reply enters the existing authenticated
durable turn path exactly once; native code never runs a model, sandbox, or second agent loop.

## P6.4 Contract

- [ ] Define versioned notification routing data for the exact session or approval without placing
      message text, secrets, bearer credentials, or other sensitive content in the push payload.
- [ ] Deep-link notification taps to the routed session/approval after device authentication where
      required, with a safe fallback when the target no longer exists or authority was revoked.
- [ ] Add Android inline reply for eligible session notifications. Accept only non-empty sanitized
      text within an explicit UTF-16/code-point and encoded-byte bound.
- [ ] Treat pressing the notification Send action as the owner's explicit send confirmation; do not
      reopen Flutter for a second confirmation.
- [ ] Submit through the existing authenticated durable message API with a stable client message id
      and session id. Native code must not execute the agent loop or invent another turn contract.
- [ ] Persist only the minimum bounded retry state needed across process death; clear it after a
      terminal response, revocation, permanent failure, or expiry.
- [ ] Preserve Approve once / Deny behavior, Android 12+ device authentication, generic lock-screen
      copy, resolution cancellation, leased push delivery, and default-deny semantics.
- [ ] Surface bounded success/failure feedback without exposing reply text on the lock screen or
      silently falling back to a different session.

## Deterministic Verification

- [ ] Pure Kotlin tests cover route decoding, text sanitization/bounds, stable id generation,
      duplicate delivery, retry, expiry, revoked auth, missing session, and permanent failure.
- [ ] Flutter tests cover deep-link restoration and replay without creating a second user message or
      showing a stale composer/preview.
- [ ] Server/API tests prove duplicate client message ids are idempotent and revoked device tokens
      cannot enqueue a turn.
- [ ] Flutter analyze, the full Flutter suite, focused Android unit tests, and affected Rust/Web gates
      pass.
- [ ] Merged-manifest and APK inspection prove only the intended receiver/service/actions are
      exported and the release ABI/signing contracts remain intact.

## Physical And Live Closeout

- [ ] Build an arm64 release APK with the established signing certificate and install it in place on
      the paired Android 15 device without losing credentials or session state.
- [ ] Prove notification taps restore the exact context from foreground, background, and killed
      states.
- [ ] Prove distinct inline replies each create one durable user message and one turn, including a
      killed-process send followed by cold-start recovery.
- [ ] Prove duplicate intent/delivery and transient retry do not duplicate the message or turn.
- [ ] Prove expired/revoked credentials, deleted sessions, empty/oversized input, cancellation, and
      permanent server failure send nothing and fail visibly.
- [ ] Record replayable evidence and align product §27, `ROADMAP.md`, `TODO.md`, and `AGENTS.md`
      before marking P6.4 complete.

## Regression Invariants

- Preserve pairing, secure storage, HTTPS-only release networking, backup exclusion, release signing,
  stable production push encryption, UnifiedPush/ntfy, and Firebase/FCM absence.
- Preserve review-before-send for Sharesheet and selected-text imports; notification Send is a
  separate explicit owner action, not automatic intent receipt.
- Keep imported/replied text untrusted and authority-free. It grants no capability, resource, URI,
  pairing, approval, filesystem, or network authority.
- Reuse the durable turn queue, event log, SSE replay, and server-owned session authority.
- Keep OMP ACP replaceable and native Deno as the coding dogfood path.

## Queued After P6.4

1. P6.5 quick capture: launcher shortcut, then Quick Settings; widget only through the same bounded
   capture intent.
2. P6.6 self-hosted voice capture and formal P6 closeout.
3. P8 fuller memory with self-hosted Postgres FTS + pgvector hybrid recall.
4. P7.2b Auto-mode persona proposals with durable manual activation and rollback.
5. P9 production egress and opaque secret broker.
6. P10 MCP catalog and live research through the P9 boundary.

## Demand-Triggered Or Permanently Out

- Demand-triggered: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and additional sandbox
  backends.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.
