# TempestMiku client contracts

The previous Flutter UI has been intentionally removed for a clean rewrite. This package currently
contains only the reusable client boundary:

- `lib/miku_api.dart` is the public library entrypoint.
- `MikuSessionClient` and its native, Web, and scripted implementations own HTTP/SSE transport.
- `session_models.dart` owns the wire models shared by future UI code.
- notification, share import, voice capture, and local ASR files retain the platform bridges.
- contract tests cover transport, parsing, idempotency, credentials, voice, and platform boundaries.

There is deliberately no `lib/main.dart`, screen, theme, widget library, or Web shell. Add a new app
entrypoint only when the replacement UI is ready to define its own structure.
