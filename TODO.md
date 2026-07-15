# TODO

Last aligned: **2026-07-15**.

Active milestone: **P6.6 — self-hosted voice capture and P6 closeout**.

`ROADMAP.md` is the milestone-order and completion-history source. The approved long-range route is
captured in `plans/one-shot-future-roadmap.md`. This file contains only the active executable slice,
its regression boundaries, and the queued order.

## P6.6 Outcome

An Android owner can record bounded audio, receive a transcript from a replaceable self-hosted
Whisper-compatible service on lumo, edit it, and explicitly choose the current or a new session
before sending. Recording or transcription never sends a message, grants model authority, or creates
a second agent loop.

## P6.6 Contract

- [ ] Freeze the audio contract: supported format, sample rate/channels, maximum duration and bytes,
      request timeout, cancellation, retry, retention, and deletion behavior.
- [ ] Add explicit Android microphone permission and a visible record/stop/cancel flow; do not add
      background recording, hotword listening, call capture, or ambient audio authority.
- [ ] Upload only bounded audio over the existing origin-bound authenticated HTTPS path. Reject
      wrong MIME/format, oversized or empty input, redirects, revoked credentials, and stale work.
- [ ] Put transcription behind a narrow replaceable server/provider interface and deploy the
      required Whisper-compatible provider on lumo. Platform or third-party recognition may be an
      explicit fallback only, never the production dependency.
- [ ] Keep raw audio ephemeral: never place it in prompts, transcripts, SSE, logs, notifications,
      memory, Drive, or artifacts; delete it after success, terminal failure, expiry, or cancellation.
- [ ] Present the returned text in an editable review sheet with no preselected destination and
      explicit current/new-session confirmation before reusing the durable message path.
- [ ] Make retries and process recovery idempotent so provider timeout, reconnect, repeated results,
      or process death cannot duplicate a transcript preview, session, message, or turn.
- [ ] Keep native Android and the transcription service free of sandbox, agent-loop, pairing,
      approval, filesystem, arbitrary-network, and automatic-send authority.

## Deterministic Verification

- [ ] Server/provider tests cover auth, format and size bounds, timeout/cancel, retry/idempotency,
      redirect refusal, revocation, provider failure, cleanup, and no retained or logged audio.
- [ ] Flutter/Android tests cover permission denial, record/stop/cancel, duration and byte bounds,
      editable transcript review, explicit current/new routing, no-auto-send, and cold/warm recovery.
- [ ] Flutter analyze/full tests, focused Android unit tests, affected Rust/Web gates, strict Rust
      format/clippy/tests, and deployment configuration checks pass.
- [ ] Merged-manifest and APK inspection preserve exported-component boundaries, arm64 release ABI,
      established signing, HTTPS-only networking, backup exclusion, push behavior, and Firebase absence.

## Physical And Live Closeout

- [ ] Deploy the replaceable self-hosted transcription provider on lumo and prove health, bounded
      latency, restart recovery, cancellation, cleanup, and visible degradation when it is unavailable.
- [ ] Install the established signed arm64 release in place on the paired Android 15 device without
      losing credentials, sessions, push registration, or prior transcript state.
- [ ] Prove record/edit/cancel and permission-denial paths, plus distinct current/new-session sends
      that each create exactly one durable user message and turn.
- [ ] Prove background/foreground transitions, provider retry, process death, and cold start do not
      retain audio or replay a transcript, message, session, or turn.
- [ ] Record replayable evidence, align product §27, running docs, `ROADMAP.md`, `TODO.md`, and
      `AGENTS.md`, and close P6 only after every deterministic, live, and signed physical gate passes.

## Regression Invariants

- Preserve pairing, secure storage, HTTPS-only release networking, backup exclusion, release signing,
  stable production push encryption, UnifiedPush/ntfy, actionable notifications, quick capture,
  Sharesheet/selected-text review, and Firebase/FCM absence.
- Treat audio and transcripts as untrusted, authority-free input. They grant no capability, resource,
  URI, pairing, approval, filesystem, secret, or network authority.
- Reuse the durable turn queue, stable client ids, event log, SSE replay, and server-owned session
  authority only after explicit Send.
- Keep the self-hosted provider replaceable, OMP ACP replaceable, and native Deno as the coding
  dogfood path.

## Queued After P6.6

1. P8 fuller memory with self-hosted Postgres FTS + pgvector hybrid recall.
2. P7.2b Auto-mode persona proposals with durable manual activation and rollback.
3. P9 production egress and opaque secret broker.
4. P10 MCP catalog and live research through the P9 boundary.

## Demand-Triggered Or Permanently Out

- Demand-triggered: cloud drive sync/CRDT, `code.ast`, `code.lsp`, `tm-trace`, and additional sandbox
  backends.
- Permanently out: aggressive autonomous rewrites, automatic persona activation, raw `SOUL.md`
  mutation, raw shell/ambient authority, Firebase/FCM, and chat-native tool sprawl.
