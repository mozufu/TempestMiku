# tm-e2e

`tm-e2e` is the local/dev E2E hatch for letting an LLM-driven actor speak to
Miku through the same HTTP/SSE session API as the Flutter/Web clients.

It does not add a production endpoint or model-visible capability. The driver
only calls:

- `POST /sessions`
- `POST /sessions/:id/messages`
- `GET /sessions/:id/events`
- approval, memory, project, and resource routes

## Scripted Run

Start `tm-server`, then run:

```sh
cargo run -p tm-e2e -- scripted
```

Useful environment:

```sh
TM_MIKU_BASE_URL=http://127.0.0.1:8787
TM_MIKU_BEARER_TOKEN=...
TM_MIKU_E2E_TIMEOUT_MS=15000
TM_E2E_REQUIRE_ARTIFACT=1
```

## Live Speaker Run

Live mode uses an OpenAI-compatible model as the outside test actor. It is
explicitly opt-in so normal tests stay network-free:

```sh
TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... OPENAI_MODEL=... cargo run -p tm-e2e -- live
```

Use `TM_E2E_SPEAKER_MODEL` to choose a separate model for the E2E actor.

## Coverage Boundary

The workflow verifies the public P1/P2 surface: session creation, Miku persona
metadata, SSE streaming and replay, mode routing, memory approval/persistence,
project promotion, and resource reads. The full native Deno engineering path
remains covered by the existing focused server tests for `fs.*`, `code.*`,
`proc.*`, artifacts, and approval approve/deny/timeout behavior.
