# TempestMiku Flutter client

This package contains the shared Web/Android client boundary and the current companion-first UI:

- `lib/miku_api.dart` is the public library entrypoint.
- `MikuSessionClient` and its native, Web, and scripted implementations own HTTP/SSE transport.
- `session_models.dart` owns the shared wire models.
- `conversation_screen.dart` keeps chat as the primary surface; low-frequency navigation lives in a
  left drawer and per-session controls live in a right context drawer.
- History, Project, Drive, Resources, and reviewed memory/evolution changes use real client
  contracts with bounded loading, empty, error, retry, and ended-session states.
- Durable turn receipts/recovery, protected readiness, device management, approvals, Mode controls,
  and correlated runtime activity are rendered without turning model-side SDK capabilities into
  ambient UI authority.
- Settings persists an explicit system/light/dark choice locally and marks the origin-bound current
  auth device without guessing for older credentials; the current row uses the dedicated logout
  flow instead of offering self-revocation.
- Text-compatible non-`skill://` resources can be expanded in exact 200-line selector pages; a
  failed later page keeps the already loaded content and exposes an explicit retry.
- Reviewed memory, persona/mode guidance, and mode/persona/skill rollback flows only create durable
  proposals. The inline approval remains the manual apply boundary, and exact rollback digests are
  shown again before submission.
- Foreground voice capture, verified local-model install/delete, explicit self-hosted ASR selection,
  and share/selected-text/quick-capture imports all enter the same editable, no-auto-send review.
- Notification, share import, voice capture, and local ASR integrations retain their native platform
  boundaries; current notification registration state is described only from local permission,
  local opt-in, and the current launch's server receipt.
- Contract and widget tests cover transport, parsing, idempotency, credentials, durable turns,
  readiness, resource paging, reviewed proposals/rollbacks, keyed events, notification authority
  transitions/actions, voice, platform boundaries, responsive chat, drawers, projects, Drive,
  approvals, and Mode controls.

The checked-in replacement UI accepts either a pasted one-time pairing link or an Android camera QR
scan. A scan only fills the same strict v1 review flow; it never changes the server or exchanges the
code until the owner confirms the displayed origin. Local Flutter/software gates do not constitute
a new physical Android acceptance run; the signed P6 device evidence remains historical evidence.

Run the package gates from this directory through the repository dev shell:

```sh
nix develop --command flutter analyze
nix develop --command flutter test
nix develop --command flutter build web
nix develop --command flutter build apk --debug --target-platform android-arm64
```
