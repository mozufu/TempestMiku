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

Every run writes a JSON conversation-round record. By default it lands at:

```sh
target/tm-e2e/<mode>-latest.json
```

Override the path with either form:

```sh
cargo run -p tm-e2e -- scripted --record-json /tmp/tm-e2e.json
TM_E2E_RECORD_PATH=/tmp/tm-e2e.json cargo run -p tm-e2e -- scripted
```

Override the scripted user messages with CLI flags:

```sh
cargo run -p tm-e2e -- scripted \
  --personal-message "what tools are available to you?" \
  --coding-message "fix a small Rust bug and state the decision"
```

Useful environment:

```sh
TM_MIKU_BASE_URL=http://127.0.0.1:8787
TM_MIKU_BEARER_TOKEN=...
TM_MIKU_E2E_TIMEOUT_MS=15000
TM_E2E_REQUIRE_ARTIFACT=1
TM_E2E_RECORD_PATH=target/tm-e2e/scripted-latest.json
```

The JSON record is machine-readable so UI tests and manual dogfood runs can
compare rounds without scraping logs:

```json
{
  "schemaVersion": 1,
  "mode": "scripted",
  "sessionId": "...",
  "rounds": [
    {
      "index": 1,
      "step": "personal_assistant_greeting",
      "userMessage": "hello Miku...",
      "assistantStreamedText": "...",
      "assistantFinalText": "...",
      "mode": "personal_assistant",
      "eventIdStart": 2,
      "eventIdEnd": 4,
      "eventTypes": ["text", "final"],
      "resourceUris": []
    }
  ]
}
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
