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
TM_MODES_PATH=/absolute/path/to/persona-assets # optional hand-authored SOUL/modes/skills
TM_MANAGED_SKILLS_PATH=/shared/path/to/managed-skills # optional; defaults under artifact root
TM_MANAGED_MODE_ADDENDA_PATH=/shared/path/to/managed-mode-addenda # optional; defaults under artifact root
TM_MEMORY_EMBEDDING_PROVIDER=disabled # disabled or local; local requires Postgres + pgvector
TM_OMP_ACP_ENABLED=0
TM_PUSH_PROVIDER=disabled # set unifiedpush only after configuring the exact endpoint origin
```

Without `TM_DATABASE_URL`, sessions, memory, and drive metadata use non-durable in-memory stores.
Drive blobs still use the configured artifact root, but entries, attributes/tags, organizer state,
version counters, corrections, and dynamic link tombstones do not survive a restart. This historical
`InMemoryDriveStore` path remains the normal local-development exception; do not use it for a durable
deployment.

P8 hybrid recall is disabled by default. To use the self-hosted path, provision `pgvector` in the
TempestMiku database and expose an unauthenticated OpenAI-shaped embedding endpoint on loopback only:

```sh
TM_MEMORY_EMBEDDING_PROVIDER=local
TM_MEMORY_EMBEDDING_ENDPOINT=http://127.0.0.1:11434/v1/embeddings
TM_MEMORY_EMBEDDING_MODEL=bge-m3:567m
TM_MEMORY_EMBEDDING_DIMENSIONS=1024
TM_MEMORY_EMBEDDING_NORMALIZATION=l2 # l2 or none; defaults to l2
TM_MEMORY_EMBEDDING_TIMEOUT_MS=10000 # defaults to 5000
TM_MEMORY_EMBEDDING_MAX_BATCH_SIZE=16 # defaults to 32
TM_MEMORY_EMBEDDING_MAX_INPUT_BYTES=16384
```

Enabled embeddings require `TM_DATABASE_URL`. Split `api` and `worker` processes must use the same
pinned values: API roles embed queries for turn recall, and worker roles stage/reclaim durable jobs
and promote a generation only after active-record coverage is complete. The endpoint validator
accepts only plain HTTP on a loopback host without user info, query data, or fragments; the client
bypasses ambient proxies and refuses redirects. Provider loss, missing/partial generations, and
configuration mismatch remain visible as typed lexical fallback;
`openai_compatible` is rejected until the P9 egress/opaque-secret boundary exists. The checked-in
lumo deployment binds the embedding service only to `127.0.0.1:11434`; see the
[P8 closeout evidence](evidence/2026-07-15-p8-5-fuller-memory.md) for pinned provenance and gates.

Approved P7.1 skill proposals are stored as immutable digest-addressed versions beneath
`TM_MANAGED_SKILLS_PATH`; when unset, the server uses `<artifact-root>/managed-skills`. API and worker
processes in a split deployment must share the same managed-skill root just as they share the artifact
root. Activation and rollback atomically replace only the per-skill active pointer. Bundled or
`TM_MODES_PATH` hand-authored skills cannot be shadowed by managed versions.

Approved P7.2a mode proposals are stored as immutable typed addendum versions beneath
`TM_MANAGED_MODE_ADDENDA_PATH`; when unset, the server uses
`<artifact-root>/managed-mode-addenda`. API and worker processes must share this root. Activation and
rollback atomically replace only the per-mode active pointer. Addenda compose description/routing
guidance into the next prompt; they cannot alter `SOUL.md`, voice caps, capabilities, scopes, skills,
or the hand-authored `modes.json` catalog.

`api` serves HTTP only, `worker` dispatches durable turns and runs approval effects, dreams, and cron,
and `all` runs both in one process. `worker` and `all` require Postgres. A split deployment runs one
or more `api` and `worker` processes against the same Postgres database and shared artifact root.
Shutdown stops new claims, continues heartbeats while draining, and aborts remaining work after the
30-second grace period. The default `api` role without a worker can create/read state but will leave
new turns queued.

Push support is disabled by default. `TM_PUSH_PROVIDER=fake` remains debug-only. Production
UnifiedPush uses `TM_PUSH_PROVIDER=unifiedpush`, `TM_UNIFIED_PUSH_ENDPOINT_ORIGIN` set to one HTTPS
origin, and `TM_PUSH_ENCRYPTION_KEY`, a base64-encoded 32-byte key shared by split API and worker
processes. Registrations are encrypted in `device_push_registrations`; deliveries use the leased
`approval_push_deliveries` outbox. The provider rejects endpoints outside the configured origin,
does not follow redirects, encrypts routing-only payloads with RFC 8291 `aes128gcm`, and treats
404/410 as permanent endpoint loss. There is no Firebase SDK or credential path.

For the checked-in self-hosted deployment, `~/deployment-config` exposes ntfy at
`https://push.justaslime.dev`. Configure both server roles with:

```bash
TM_PUSH_PROVIDER=unifiedpush
TM_UNIFIED_PUSH_ENDPOINT_ORIGIN=https://push.justaslime.dev
TM_PUSH_ENCRYPTION_KEY="$(openssl rand -base64 32)"
```

Generate the encryption key once and persist the same value for API and worker roles; do not generate
it independently at each start. On Android, install the ntfy distributor, select the self-hosted
server, then reopen the paired TempestMiku app. The official connector registers a high-entropy
endpoint and Web Push keys through the authenticated device API. Incoming payloads must decrypt
successfully before the native service can show or cancel an approval notification.

The checked-in lumo deployment runs `TM_SERVER_ROLE=all` as a separate OpenRC/Podman service beside
Hermes, reuses the local PostgreSQL Unix socket and CLIProxyAPI, and reads the persistent encryption
key from SOPS. `https://miku.justaslime.dev/health`, the 12 ordered migrations, and state-preserving
service restart passed on 2026-07-14. The signed Android killed-process request/resolution canary
described below closes the P6.1 production-delivery gate.

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

Background approval events use a private Android notification with **Approve once** and **Deny**.
The public lock-screen version contains no action or scope. Android 12+ requires device
authentication before delivering an action; Android 6–11 opens the app for confirmation. The
action reloads the target session and calls the normal authenticated approval endpoint. To exercise
the notification while the debug app process is stopped, replace the session id with a real session
that currently has a matching pending approval:

```sh
adb shell am force-stop org.mozufu.tempestmiku
adb shell am broadcast \
  -a org.mozufu.tempestmiku.DEBUG_APPROVAL_NOTIFICATION \
  -n org.mozufu.tempestmiku/.DebugApprovalNotificationReceiver \
  --es sessionId '<session-id>' \
  --es approvalId '<approval-id>' \
  --es approvalAction 'proc.run cargo test'
```

This is an ADB/local-SSE probe, not proof of remote killed-process push delivery. The production
proof was captured on 2026-07-14 with a signed `org.mozufu.tempestmiku` release on an Android 15
physical device and ntfy Android 1.24.0 configured for `https://push.justaslime.dev`. After the
Flutter app was killed, `approval_requested` delivered at 07:32:24 +08:00 and created the private
notification; `approval_resolved` delivered at 07:33:23 after timeout and cancelled it. Each outbox
row completed in one attempt without a provider error, and the Flutter activity remained closed.
The canary release signer SHA-256 fingerprint is
`503f865843464347cb2d2f90be3ab4dbf68bae690443bf8fd262e0dcc0b58ee1`.

Session-ready pushes use a private `MessagingStyle` notification with an exact session route and a
bounded **Reply** action. On HyperOS/MIUI, enable background autostart for both Tempest Miku and the
ntfy UnifiedPush distributor and set their battery mode to unrestricted; provider acceptance alone
does not prove that the distributor delivered immediately on the phone. The public lock-screen copy
stays generic and has no reply action or transcript text.

The P6.4 signed Android 15 canary ran on 2026-07-14 with the same release fingerprint above and an
in-place `adb install -r`. `firstInstallTime` and the existing credential/transcript survived. Fresh
foreground, background, and swiped-away-process notifications opened their exact sessions without
adding a user message. Distinct inline replies and a double-tapped killed-process reply each produced
one user message and one `session_turns` row with `notification-<delivery-id>` as the client id; a
Wi-Fi-off reply stayed absent until connectivity returned, then materialized once. A forced cold
start restored the killed-process reply and assistant response. Empty Send remained disabled, cancel
and 1,001-code-point input sent nothing, and expired routes, revoked credentials, deleted sessions,
and ended sessions each produced the expected visible terminal notification with zero matching
messages. The temporarily revoked production device row was restored and a post-restore inline reply
again produced exactly one message and turn.

P6.5 uses one versioned quick-capture intent behind both the static launcher shortcut and the Quick
Settings tile. Receipt opens the existing editable review sheet; it never sends, pairs, approves,
or grants authority. The native parser accepts only a fresh UUID and optional sanitized bounded
text, and rejects MIME, data, `ClipData`, selectors, streams, URI grants, and unknown extras. The
launcher path uses a no-history trampoline, while Android 14+ tile launches use an immutable
`PendingIntent`. No widget or second native send path is present.

The P6.5 signed arm64 canary ran on Android 15 on 2026-07-15 with the same release fingerprint and
an in-place `adb install -r`; `firstInstallTime`, pairing, and prior transcript state survived. The
real launcher long-press menu exposed **Capture with Miku**, and the HyperOS control center exposed
**Miku capture**. Each opened the same empty review from foreground, background, and a killed app
process. Empty Send remained disabled and cancellation sent nothing. An edited current-session
capture advanced the existing session from two to four messages, and a distinct new-session capture
ended with exactly two messages. Re-delivering the same capture UUID opened no review and changed no
message count. Killing the process while a draft was visible, then relaunching normally, restored
the authenticated chat without replaying the draft.

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

tm-lang is the sole language/runtime for CLI and server chat/coding sessions:

```bash
cargo run -p tm-cli -- "use execute(code) to inspect the workspace"
```

Backend-selection flags and environment selectors no longer exist.

The retired 20-case x 50-run comparative runner is not a supported command. Its immutable cutover
record and frozen corpus remain under `docs/evidence/2026-07-16-tm-fluency-prompt-v2/`.

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
cargo run -p tm-cli -- --event-log .tempestmiku/tm-events.jsonl "Run with structured telemetry."
```

The CLI streams assistant text to stdout and cell/tool telemetry to stderr. `--event-log` additionally
writes one flushed JSON object per line for reasoning/text deltas, tool calls, cell starts/results,
turn boundaries, and the final answer; it remains usable after a nonzero turn-budget exit.

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
  "self_evolution": {
    "tier": "conservative"
  },
  "proc_run_timeout_ms": 180000,
  "artifact_root": ".tempestmiku/artifacts"
}
```

Notes:

- `path` must resolve to an existing directory.
- `mode` is `ro` or `rw`.
- `commands` is an allowlist of executable names for `proc.run`.
- `safe_args` entries are argv prefixes that can run without approval.
- `proc.run.stdin` accepts an optional UTF-8 string capped at 1 MiB. It shares the command timeout
  with process execution and output collection; non-string or oversized input fails before spawn.
- `proc_run_timeout_ms` sets both the default and maximum per-command timeout. It defaults to
  180000 and must stay between 1 and 900000; benchmark adapters may opt into a larger bounded value.
- On Unix, each `proc.run` command owns a fresh process group. Timeout or turn cancellation kills
  the full descendant tree before returning, so compiler/test grandchildren cannot leak into later
  gates.
- Approval mode `deny` is the default. Approval mode `manual` asks before non-safe writes or
  commands.
- `self_evolution.tier` accepts `off`, `conservative` (the compatibility-preserving default), or
  `moderate`. Existing `approvals.mode` and timeout settings remain orthogonal and still gate
  reachable writes; selecting a tier does not grant a capability or bypass approval. P7.0 rejects
  `aggressive`; moderate review never applies persona files or rewrites hand-authored mode assets.
  Authenticated `/ready` exposes only the effective tier, never proposal content or credentials.
- Moderate review candidates enter through `POST /sessions/:id/evolution/review-proposals` and are
  readable at `memory://review-proposals/<id>`. Persona proposals remain review-only. When the
  managed mode-addendum catalog is configured, a mode proposal returns `applyEnabled: true`; approval
  installs an immutable guidance-only version, and
  `POST /sessions/:id/evolution/modes/:mode_id/rollback` creates a separate durable rollback approval.
- The CLI prompts on the tty; the server emits approval events to the UI/API.
- There is no raw shell escape hatch. Commands run as argv vectors.
- Benchmark adapters may pass `--turn-budget-ok`: the CLI records a
  `turn_budget_exhausted` JSONL event and exits successfully after the final cell result. Without
  that opt-in flag, turn-budget exhaustion remains a nonzero CLI error.

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

For the offline native tm coding-backend gate (linked-repo patch, targeted
test, artifact spill, approval approve/deny/timeout, and durable turn replay):

```sh
nix develop --command cargo run -p tm-e2e -- record native-coding
```

For the P7.0 public self-evolution policy record (Conservative allow/deny, Moderate review-only
approval, timeout, downgrade decision, forged target, retry idempotency, bounded resource spill,
and replay continuity):

```sh
nix develop --command cargo run -p tm-e2e -- record evolution-policy
```

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
- A pre-restart ACP/native-runtime approval is cancelled: those waits cannot safely resume after their origin is
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
