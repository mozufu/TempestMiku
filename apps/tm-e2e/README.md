# tm-e2e

`tm-e2e` is the local/dev E2E hatch for letting an LLM-driven actor speak to
Miku through the same HTTP/SSE session API as the Flutter/Web clients.

It does not add a production endpoint or model-visible capability. The driver
only calls:

- `POST /sessions`
- `POST /sessions/:id/messages` with a unique `clientMessageId` (durable `202` turn)
- `GET /sessions/:id/turns/:turnId`
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

External-server runs authenticate every HTTP/SSE request with `TM_MIKU_BEARER_TOKEN`. The SSE parser
accepts only `event: session_event`, deduplicates durable numeric ids, and reads the logical event kind
from `data.type`; the `eventTypes` arrays below contain those logical type values, not wire event names.

`tm-e2e` loads the nearest workspace `.env` before reading these variables. Values already exported
in the shell win over `.env`. Evidence manifests include credential presence only as redacted
environment entries (`OPENAI_API_KEY`, `TM_MIKU_BEARER_TOKEN`, etc.).

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

## Recording Evidence Runs

The recording pipeline is the preferred local/dev E2E gate. It reuses the same
public HTTP/SSE session API, but writes a full evidence bundle instead of only a
round summary:

```sh
cargo run -p tm-e2e -- record suite
```

The default output lands at:

```sh
target/tm-e2e/runs/<timestamp>-suite/
```

Useful variants:

```sh
cargo run -p tm-e2e -- record api
cargo run -p tm-e2e -- record ui --headed
TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... cargo run -p tm-e2e -- record live-api
TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... cargo run -p tm-e2e -- record native-actor
```

The normal `suite` run starts an in-process `tm-server` fixture, uses the
deterministic echo/scripted backends, drives the real Flutter Web UI through
Playwright, and stays network-free. If the Flutter Web build already exists,
skip rebuilding it with:

```sh
TM_E2E_SKIP_FLUTTER_BUILD=1 cargo run -p tm-e2e -- record suite
```

Each evidence bundle includes:

- `manifest.json` — schema v2 run metadata, git state, sanitized env, scenario
  statuses, server config, and artifact paths.
- `events.ndjson` — every SSE event observed by the Rust client path.
- `http.ndjson` — public API requests/responses observed by the Rust client path.
- `transcript.md` — readable scenario timeline.
- `resources/` — captured previews/resolved resource envelopes.
- `ui/` — Playwright screenshots, videos/traces, console logs, network logs, and
  UI result JSON.
- `report.md` and `index.html` — human-openable summaries.

## Actor Smoke

`tm_e2e::run_actor_smoke` is a narrow public-API smoke used by tests for the
P3+ actor surface. It creates a Handoff session, watches actor lifecycle events
over SSE, resolves a child `native-deno` approval through
`POST /sessions/:id/approvals/:approval_id`, opens child `artifact://`,
`history://`, and `agent://` resources through the session resource gateway,
checks a terminal cancelled `agent://` record, and reconnects with
`Last-Event-ID` to prove replay includes approval, output-link, completion, and
cancellation events.

## Drive Smoke

`tm_e2e::run_drive_smoke` is the P5 public-API smoke for the local-first drive
and research workspace. It starts a Serious Engineer session, sends a scripted
native-Deno turn that files a dropped `drop://` document through `drive.put`,
watches the shared approval appear in transcript `pendingEvents`, approves it
through the normal approval route, then verifies `drive_put` replay, `drive://`
preview/resolve, the compact drive feed, `drive.search`, `research.drive`, and
`Last-Event-ID` replay.

## Native Actor Coordination

`native_deno_actor_coordination_public_api_covers_p3_plus_route` is the
network-free public-API E2E for the native Deno actor path. It starts an
in-process `tm-server` with `NativeDenoBackend`, injects a scripted streaming
LLM, opens a Handoff session through HTTP, and runs real sandbox SDK calls:
`agents.spawn`, `agents.send`, `agents.broadcast`, `agents.wait`, and
`agents.list`. The test verifies live SSE plus `Last-Event-ID` replay for
`actor_spawned`, `actor_message`, `actor_completed`, `actor_resources_linked`,
and `final`, then resolves each child `artifact://`, `history://`, and
`agent://` resource through the public session resource gateway.

For a credentialed live check without letting the model free-form the JS route,
run:

```sh
TM_LLM_E2E_LIVE=1 cargo run -p tm-e2e -- record native-actor
```

That command loads `.env`, performs a real OpenAI-compatible streaming preflight,
uses the same native Deno actor route, and lets the final parent/child LLM turns
come from the `.env` endpoint while keeping the executed JS deterministic.

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
project promotion, and resource reads. Actor smoke verifies the public P3+
attach/approve/resource/replay path, while native actor coordination verifies
the real Deno SDK route for P3+ mailbox coordination and child resources. The
drive smoke verifies the P5 drop/approve/file/preview/search/research/replay
path. The remaining native Deno engineering path stays covered by focused server
tests for `fs.*`, `code.*`, `proc.*`, child actor approval routing, and approval
approve/deny/timeout behavior.
