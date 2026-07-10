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

For a functional local server, use Postgres and the combined API/worker role:

```sh
TM_DATABASE_URL=postgres://user:password@127.0.0.1:5432/tempestmiku \
TM_SERVER_ROLE=all \
nix develop --command cargo run -p tm-server
```

Open:

```text
http://127.0.0.1:8787/pair
```

For the first loopback browser, click **Pair this web browser**, then continue to `/`. The server
sets an HttpOnly device cookie; application API calls are not anonymously accessible.

`tm-server` serves `clients/miku_flutter/build/web` by default. To serve a different build:

```sh
TM_WEBUI_DIR=/absolute/path/to/web-build nix develop --command cargo run -p tm-server
```

Useful server variables:

```sh
TM_SERVER_ADDR=127.0.0.1:8787
TM_DATABASE_URL=postgres://user:password@host:5432/dbname
TM_SERVER_ROLE=api # api, worker, or all; defaults to api
TM_AUTH_MODE=device
TM_OWNER_SUBJECT=brian
TM_PUBLIC_BASE_URL=https://miku.example.test # production external origin only
TM_HOST_CONFIG=.tempestmiku/config.json
TM_CONFIG=.tempestmiku/config.json
TM_OMP_ACP_ENABLED=0
```

Without `TM_DATABASE_URL`, sessions, memory, and drive metadata use non-durable in-memory stores.
Drive blobs still use the configured artifact root, but entries, attributes/tags, organizer state,
version counters, corrections, and dynamic link tombstones do not survive a restart. This historical
`InMemoryDriveStore` path remains the normal local-development exception; do not use it for a durable
deployment.

`api` serves HTTP only, `worker` dispatches durable turns and runs approval effects, dreams, and cron,
and `all` runs both in one process. `worker` and `all` require Postgres. A split deployment runs one
or more `api` and `worker` processes against the same Postgres database and shared artifact root.
Shutdown stops new claims, continues heartbeats while draining, and aborts remaining work after the
30-second grace period. The default `api` role without a worker can create/read state but will leave
new turns queued.

Postgres startup applies ordered, checksummed migrations from `crates/tm-server/migrations` and
preserves existing session/memory history. A checksum mismatch or failed migration aborts startup;
do not recreate the schema to upgrade. Protected `GET /ready` and `GET /metrics` expose migration,
worker, queue-age, scheduler-lag, lease, approval, and link-hydration state.

### Production deployment and hardening status

The audit hardening implementation has landed: durable turns, approvals/effects, fenced dream and
cron leases, supervised workers, authoritative session scopes, Postgres drive metadata/tombstones,
and restart-safe SSE replay are wired. The local Rust, clean-schema/two-process Postgres, Flutter,
Playwright, signed Android APK, certificate, RustSec, and secret verification matrix passes. A
physical Android 15 release canary over Tailscale Serve HTTPS also passes: in-app QR confirmation,
durable chat, cold-start credential/session recovery, exact-once replay of a turn completed while
offline, and immediate SSE/API loss after device revocation. The audit hardening gate is closed for
the documented single-owner deployment.

Production `tm-server` must stay on loopback behind an HTTPS reverse proxy or Tailscale Serve:

```sh
TM_SERVER_ADDR=127.0.0.1:8787 \
TM_SERVER_ROLE=all \
TM_DATABASE_URL=postgres://user:password@host:5432/dbname \
TM_AUTH_MODE=device \
TM_PUBLIC_BASE_URL=https://miku.example.test \
nix develop --command cargo run -p tm-server
```

Configure the proxy/Tailscale service to terminate HTTPS and forward to `127.0.0.1:8787`. Do not
bind production HTTP to `0.0.0.0`; `TM_PUBLIC_BASE_URL` describes the external origin but does not
add TLS. A non-loopback bind is rejected unless `TM_ALLOW_INSECURE_HTTP=1` is used in a debug build,
which is only for emulator development. Forwarded identity/host/protocol headers are accepted only
in forwarded-auth mode and only from `TM_AUTH_TRUSTED_PROXY_CIDRS`.

With Postgres, drive entries, attributes/tags, organizer proposals/runs, corrections, versions,
links, and revocation tombstones survive restart. Active roots are revalidated against their persisted
canonical identity; missing, moved, symlink-replaced, revoked, or invalid roots remain disabled. The
no-database in-memory metadata path is a historical local-development exception: old in-memory drive
metadata cannot be reconstructed from blobs, and Postgres persistence is forward-authoritative.

## Android Debug Client

The Android app is the same Flutter remote-control client as Web/PWA: it connects to `tm-server` over
authenticated HTTP/SSE, sends discrete POST controls, and keeps execution on the server.

Debug/profile builds may use cleartext for emulator development and default to
`http://10.0.2.2:8787` when unpaired. Release builds reject cleartext and require an HTTPS pairing
origin. Manual URL-only target changes are disabled.

For a physical Android device, keep `tm-server` on loopback and expose it through an HTTPS reverse
proxy or Tailscale Serve. Set `TM_PUBLIC_BASE_URL` to that HTTPS origin, then open the pairing page
locally on the server host:

```sh
TM_SERVER_ADDR=127.0.0.1:8787 \
TM_SERVER_ROLE=all \
TM_DATABASE_URL=postgres://user:password@host:5432/dbname \
TM_PUBLIC_BASE_URL=https://miku.example.test \
nix develop --command cargo run -p tm-server
```

On that host, open:

```text
http://127.0.0.1:8787/pair
```

The page creates a cryptographically random, single-use code that expires after five minutes. In the
Android app choose **More → Server target → Scan QR**. The QR contains versioned
`tempestmiku://pair?...` data, but Android intentionally exports no intent filter for it: pairing is
scanner-only, never a tap/deep-link handoff. Before exchange the app displays the HTTPS origin,
scheme, host, effective port, and proposed device name for confirmation.

The issued device token is origin-bound and stored with `flutter_secure_storage` 10.3.1. It is sent
as `Authorization: Bearer` on every API and SSE request. Server URL/session cursor remain ordinary
preferences, but changing servers clears credential, session, and cursor before publishing the new
target. Android API level 23 is the minimum; credential storage is excluded from backup/device
transfer. QR scanning uses bundled `mobile_scanner` 7.2.0 and the camera permission, so it works
offline against the displayed code.

```sh
nix develop --command bash -lc \
  'cd clients/miku_flutter && flutter build apk --debug'
```

The debug APK is written to `clients/miku_flutter/build/app/outputs/flutter-apk/app-debug.apk`.
If `adb` is not on `PATH`, use the SDK copy directly:

```sh
~/Library/Android/sdk/platform-tools/adb devices
```

Release builds never fall back to the debug key. Create the untracked
`clients/miku_flutter/android/key.properties` with `storeFile`, `storePassword`, `keyAlias`, and
`keyPassword`, then build and inspect its certificate:

```sh
nix develop --command bash -lc \
  'cd clients/miku_flutter && flutter build apk --release'
~/Library/Android/sdk/build-tools/<version>/apksigner verify --print-certs \
  clients/miku_flutter/build/app/outputs/flutter-apk/app-release.apk
```

Verify the reported release signer is the intended certificate and is not Android's debug signer.

## API Smoke

Health check:

```sh
curl -s http://127.0.0.1:8787/health
```

`/health` is deliberately minimal and public. All state, readiness, metrics, and SSE routes require
authentication. For a first loopback device, create and exchange a five-minute code (the JSON fields
shown as shell placeholders can also be copied from `http://127.0.0.1:8787/pair`):

```sh
curl -s -X POST http://127.0.0.1:8787/auth/pairing-codes

curl -s \
  -H 'content-type: application/json' \
  -d '{"code":"<one-time-code>","deviceName":"curl smoke","platform":"cli"}' \
  http://127.0.0.1:8787/auth/pair
```

Copy the returned `token` into `TOKEN`; it is shown only once and only its SHA-256 hash is stored.
Device-authenticated Web uses the same device table through a `Secure`, `HttpOnly`,
`SameSite=Strict` cookie. Cookie-authenticated mutations must include a matching `Origin` header;
bearer requests are not subject to the cookie CSRF check.

Create a session:

```sh
TOKEN='<device-token>'
curl -s -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"scope":"global"}' \
  http://127.0.0.1:8787/sessions
```

The response includes an `id`. Use it in the next commands.

Watch the event stream in one terminal:

```sh
curl -N \
  -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8787/sessions/<session-id>/events
```

Send a message in another terminal:

```sh
curl -s \
  -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"clientMessageId":"curl-smoke-001","content":"hello Miku"}' \
  http://127.0.0.1:8787/sessions/<session-id>/messages
```

The response is `202` with `{turnId, clientMessageId, status:"queued"}`. Retrying the same id and
content returns the same turn; reusing it with different content returns `409`. Poll status with:

```sh
curl -s \
  -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8787/sessions/<session-id>/turns/<turn-id>
```

SSE stays connected across turn finals and emits only `event: session_event` frames. Each durable
numeric `id` wraps `data: {type, turnId, payload, createdAt}`; reconnect with `Last-Event-ID`. The
stream closes only after `session_end`.

Fetch the stored transcript:

```sh
curl -s \
  -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8787/sessions/<session-id>/messages
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
- A submitted turn remains queued: the default role is `api`; run `TM_SERVER_ROLE=all` or a separate
  `TM_SERVER_ROLE=worker` process against the same Postgres database.
- A protected route returns `401`/`403`: pair a device and send its bearer token, or use the paired
  browser cookie with a matching `Origin` on mutations. Revoked devices lose API and SSE access
  immediately.
- Sessions disappear after restart: set `TM_DATABASE_URL` for Postgres session/event persistence.
- Drive entries or dynamic links disappear after restart: confirm `TM_DATABASE_URL` is set. Without it,
  drive metadata intentionally uses the historical in-memory local-development path. If a persisted
  linked root is missing or its canonical identity changed, startup marks it invalid instead of
  restoring authority; explicitly link the intended root again after inspecting the change.
- A pre-restart ACP/V8 approval is cancelled: those waits cannot safely resume after their origin is
  lost. Durable proposal effects remain in the approval outbox and resume exactly once.
- Startup rejects a migration checksum: do not edit an applied migration; add the next ordered
  migration or restore the exact applied file.
- Android rejects the server URL: release builds require HTTPS. Keep raw HTTP limited to debug/profile
  emulator development, and pair again after changing origins.
- Android release build fails before compilation: provide `android/key.properties`; release builds
  intentionally refuse debug-signing fallback.
- Code tools say no linked folders are configured: add a host config and set `TM_CONFIG` or
  `TM_HOST_CONFIG`.
- An OpenAI-compatible endpoint rejects the request body: try `OPENAI_STREAM_USAGE=0`.
