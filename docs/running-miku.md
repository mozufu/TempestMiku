# Running Miku

This guide covers the local development run paths for TempestMiku:

- the browser app served by `tm-server`
- the HTTP/SSE API
- the one-shot `tm` CLI
- the local e2e harness

Run commands from the repository root unless a step says otherwise.

## Prerequisites

The repository flake provides the expected Rust, Flutter, JDK, and font tooling. The most
repeatable path is to run commands through Nix:

```sh
nix develop --command cargo test
```

For repeated manual work, entering the shell is fine:

```sh
nix develop
```

Without Nix, use a Rust toolchain new enough for edition 2024 and a Flutter toolchain compatible
with `clients/miku_flutter/pubspec.yaml`.

## Model Configuration

`tm-server` loads a `.env` file from the current working directory before reading environment
variables. The `tm` CLI reads only the process environment, so export the same variables in the
shell when using the CLI.

Live model variables:

```sh
OPENAI_API_KEY=...
OPENAI_MODEL=gpt-4o-mini
# Optional. Defaults to https://api.openai.com/v1.
OPENAI_BASE_URL=https://api.openai.com/v1
# Optional. Use for OpenAI-compatible servers that reject stream_options.include_usage.
OPENAI_STREAM_USAGE=0
```

If both `OPENAI_API_KEY` and `OPENAI_BASE_URL` are unset or empty, `tm-server` starts in local echo
mode. Echo mode is useful for UI/API smoke tests, but it is not live Miku. The CLI always uses the
OpenAI-compatible client and needs a reachable endpoint.

## Browser App

Build the Flutter Web client:

```sh
nix develop --command bash -lc 'cd clients/miku_flutter && flutter build web'
```

Start the server:

```sh
nix develop --command cargo run -p tm-server
```

Open:

```text
http://127.0.0.1:8787
```

`tm-server` serves `clients/miku_flutter/build/web` by default. To serve a different build:

```sh
TM_WEBUI_DIR=/absolute/path/to/web-build nix develop --command cargo run -p tm-server
```

Useful server variables:

```sh
TM_SERVER_ADDR=127.0.0.1:8787
TM_DATABASE_URL=postgres://user:password@host:5432/dbname
TM_HOST_CONFIG=.tempestmiku/config.json
TM_CONFIG=.tempestmiku/config.json
TM_OMP_ACP_ENABLED=0
```

Without `TM_DATABASE_URL`, sessions and memory use the non-durable in-memory store. That is the
normal local default.

## API Smoke

Health check:

```sh
curl -s http://127.0.0.1:8787/health
```

Create a session:

```sh
curl -s -X POST http://127.0.0.1:8787/sessions
```

The response includes an `id`. Use it in the next commands.

Watch the event stream in one terminal:

```sh
curl -N http://127.0.0.1:8787/sessions/<session-id>/events
```

Send a message in another terminal:

```sh
curl -s \
  -H 'content-type: application/json' \
  -d '{"content":"hello Miku"}' \
  http://127.0.0.1:8787/sessions/<session-id>/messages
```

Fetch the stored transcript:

```sh
curl -s http://127.0.0.1:8787/sessions/<session-id>/messages
```

## CLI

Run a one-shot Serious Engineer turn:

```sh
OPENAI_API_KEY=... OPENAI_MODEL=gpt-4o-mini \
  nix develop --command cargo run -p tm-cli -- "Help me inspect this repo."
```

Useful CLI options:

```sh
cargo run -p tm-cli -- --help
cargo run -p tm-cli -- --model gpt-4o-mini "Explain the current architecture."
cargo run -p tm-cli -- --config .tempestmiku/config.json --session-id cli-smoke "Run a safe repo check."
```

The CLI streams assistant text to stdout and cell/tool telemetry to stderr.

## Host Config For Code Execution

By default, `fs.*`, `code.*`, and `proc.*` fail closed because no linked folders are configured.
Create `.tempestmiku/config.json`, or point `TM_CONFIG` / `TM_HOST_CONFIG` at another JSON file.

Minimal repo-linked config:

```json
{
  "linked_folders": [
    {
      "name": "repo",
      "path": ".",
      "mode": "rw",
      "commands": ["cargo"],
      "safe_args": [["cargo", "test"]]
    }
  ],
  "approvals": {
    "mode": "manual",
    "timeout_ms": 60000
  },
  "artifact_root": ".tempestmiku/artifacts"
}
```

Notes:

- `path` must resolve to an existing directory.
- `mode` is `ro` or `rw`.
- `commands` is an allowlist of executable names for `proc.run`.
- `safe_args` entries are argv prefixes that can run without approval.
- Approval mode `deny` is the default. Approval mode `manual` asks before non-safe writes or
  commands.
- The CLI prompts on the tty; the server emits approval events to the UI/API.
- There is no raw shell escape hatch. Commands run as argv vectors.

## E2E Harness

For a scripted public-API run against an already running `tm-server`:

```sh
nix develop --command cargo run -p tm-e2e -- scripted
```

For the full local evidence suite, including an in-process server fixture and Flutter Web UI
scenario:

```sh
nix develop --command cargo run -p tm-e2e -- record suite
```

The UI part of `record suite` runs `npm exec playwright` from `clients/miku_web`. If
`clients/miku_web/node_modules` is absent, install the Node dependencies in that directory first.

For a credentialed live speaker run:

```sh
TM_LLM_E2E_LIVE=1 OPENAI_API_KEY=... OPENAI_MODEL=gpt-4o-mini \
  nix develop --command cargo run -p tm-e2e -- live
```

Evidence bundles are written under `target/tm-e2e/`.

## Troubleshooting

- Browser shows a missing app shell: build Flutter Web, or set `TM_WEBUI_DIR` to a valid build.
- Responses look like echo output: `tm-server` did not see `OPENAI_API_KEY` or `OPENAI_BASE_URL`.
- Port `8787` is busy: set `TM_SERVER_ADDR=127.0.0.1:<port>`.
- Sessions disappear after restart: set `TM_DATABASE_URL` for Postgres persistence.
- Code tools say no linked folders are configured: add a host config and set `TM_CONFIG` or
  `TM_HOST_CONFIG`.
- An OpenAI-compatible endpoint rejects the request body: try `OPENAI_STREAM_USAGE=0`.
