# P6.4 Actionable Android Notifications Brief

Aligned: **2026-07-14**.

## Alignment Brief

- **Outcome:** An Android owner can open the exact session or approval from a notification and can
  explicitly send a bounded inline reply from an eligible session notification while Flutter is
  foregrounded, backgrounded, or killed.
- **Audience / user:** The single authenticated TempestMiku owner using the paired Android app.
- **Success criteria:** Notification routing is versioned and contains identifiers rather than
  sensitive content; Send posts exactly one durable user message through the existing authenticated
  session API; retries reuse a stable client message id; stale, revoked, expired, or permanently
  failed sends stop visibly and do not fall through to another session.
- **Must-haves:** Exact session/approval routing, explicit owner Send, bounded sanitized reply text,
  minimum process-death retry state, generic lock-screen copy, Android 12+ authentication for
  approval decisions, and deterministic Kotlin/Flutter/server coverage.
- **Non-goals:** No native model, sandbox, agent loop, alternate turn contract, automatic reply on
  intent receipt, Firebase/FCM dependency, ambient authority, or P6.5/P6.6 entry points.
- **Constraints:** Preserve secure pairing, HTTPS-only release networking, backup exclusion,
  release signing, encrypted UnifiedPush/ntfy delivery, approval default-deny behavior, and the
  existing durable turn/event/SSE authority model.
- **Key decisions:** Treat notification Send as the confirmation boundary; keep imported/replied
  text untrusted and authority-free; use a caller-owned stable client message id; never silently
  select a replacement session.
- **Implementation approach:** Extend the versioned push route and Android notification bridge,
  reuse the authenticated durable message client, restore exact Flutter context without replaying a
  user message, and persist only bounded retry metadata until a terminal outcome or expiry.
- **Acceptance checks:** Focused Kotlin, Flutter, and Rust tests; Flutter analyze/full suite; affected
  Rust/Web gates; merged-manifest and signed arm64 APK inspection; physical Android 15 foreground,
  background, killed-process, retry, revocation, expiry, and missing-session canaries before closing
  P6.4.
- **Open questions:** None for local implementation. Physical-device evidence remains an explicit
  closeout gate and must not be inferred from automated tests.

## Fresh-thread prompt

Use this plan as the project brief. First read the whole brief, then implement it. Preserve the
stated constraints, non-goals, and acceptance checks. If anything is ambiguous, ask only the
smallest blocking question before building. Keep `TODO.md`, `ROADMAP.md`, product section 27, and
`AGENTS.md` aligned only to the level supported by real evidence.
