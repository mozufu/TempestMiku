# Running Miku

This guide covers the local development run paths for TempestMiku:

- the Flutter conversation client and retained platform bridges
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
# Set only for an OpenAI-compatible proxy with broken keep-alive behavior:
OPENAI_CONNECTION_REUSE=0
```

If both `OPENAI_API_KEY` and `OPENAI_BASE_URL` are unset or empty, `tm-server` starts in local echo
mode. Echo mode is useful for UI/API smoke tests, but it is not live Miku. The CLI always uses the
OpenAI-compatible client and needs a reachable endpoint.

## Flutter client

`clients/miku_flutter` contains the runnable Web/Android app plus its HTTP/SSE clients, wire models,
credential handling, and native platform bridges. The chat-first shell connects the current
user-facing server/platform surfaces while keeping mutation authority behind explicit review and
approval boundaries; see its package README and §27.4.1 for the exact coverage map.

The API server remains runnable during the rewrite:

For a functional local server, use Postgres and the combined API/worker role:

```sh
TM_DATABASE_URL=postgres://user:password@127.0.0.1:5432/tempestmiku \
TM_SERVER_ROLE=all \
nix develop --command cargo run -p tm-server
```

The minimal server-owned pairing bootstrap remains available at:

```text
http://127.0.0.1:8787/pair
```

Build the Web app and point the existing static hosting hook at the output without changing the API
server:

```sh
cd clients/miku_flutter
nix develop --command flutter build web
cd ../..
```

```sh
TM_WEBUI_DIR="$PWD/clients/miku_flutter/build/web" nix develop --command cargo run -p tm-server
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
TM_MANAGED_PERSONA_ADDENDA_PATH=/shared/path/to/managed-persona-addenda # optional; defaults under artifact root
TM_MEMORY_EMBEDDING_PROVIDER=disabled # disabled or local; local requires Postgres + pgvector
TM_OMP_ACP_ENABLED=0
TM_PUSH_PROVIDER=disabled # set unifiedpush only after configuring the exact endpoint origin
```

The optional home-hosted voice recognizer is disabled unless all three fixed values are present:

```sh
TM_SELF_HOSTED_ASR_ENDPOINT=https://asr.example.ts.net/transcribe
TM_SELF_HOSTED_ASR_LABEL='Home TEA-ASR (Taiwan Mandarin)'
TM_SELF_HOSTED_ASR_MODEL_ID=JacobLinCool/TEA-ASR-1.1-mini
```

The endpoint is trusted operator configuration; Android never receives it and cannot replace it.
The server accepts HTTPS, or plain HTTP only when the URL host is an exact literal address inside
Tailscale's `100.64.0.0/10` CGNAT range. User info, query strings, fragments, redirects, ambient
proxies, and other private/public HTTP destinations fail startup. Omitting all three values disables
the remote engine; a partial configuration also fails startup so a misspelled label/model cannot
silently enable a weaker mode. The Flutter drawer still defaults to the on-device model and requires
an explicit disclosure before selecting this engine. Each recording is a separate foreground
upload through the paired authenticated server; failure never falls back to local recognition, and
neither audio nor transcript is persisted by `tm-server`.

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
`openai_compatible` remains rejected even though P9 now exists: the memory provider has not been
migrated to the opaque-secret broker, so enabling it would bypass the closed boundary. The checked-in
lumo deployment binds the local embedding service only to `127.0.0.1:11434`; see the
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

Approved P7.2b persona proposals are stored as immutable typed addendum versions beneath
`TM_MANAGED_PERSONA_ADDENDA_PATH`; when unset, the server uses
`<artifact-root>/managed-persona-addenda`. API and worker processes must share this root. Activation
and rollback atomically replace only the `miku` active pointer. Addenda compose bounded tone,
address, and interaction-preference guidance into every mode's next prompt; they cannot alter
`SOUL.md`, identity, safety rules, voice caps, capabilities, scopes, routes, skills, or source
configuration.

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

One validated self-hosted deployment exposes ntfy at
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

## Historical Android client runbook

The following section records the previously closed Android packaging and physical-device contract.
It is not a current build path while `lib/main.dart` and the presentation layer are absent. The
native bridges and security boundaries remain in the tree for the replacement UI to integrate.

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

P6.6 keeps voice capture foreground-only and review-only. Local is the default: install the pinned
model from the in-app drawer only after reviewing its provenance and license; the APK contains no
model weights. When the server advertises a configured home-hosted engine, the same drawer offers it
only after an explicit disclosure that audio will leave the phone for the owner's service. Both
engines open the same editable current/new-session review sheet. Cancel, an unselected destination,
a capture failure, or a recognition failure sends nothing; neither engine falls back to the other.
The final package shows
only aggregate duration/level/clipping/silence diagnostics plus its installed app/version/build type
and installed-base-APK SHA-256; it neither retains nor uploads the waveform, and fingerprint failure
never blocks transcription. If evaluating real speech, record only with explicit consent and
preserve the exact spoken reference separately so quality can be measured without keeping audio.

The exact production model retained its verified state across force-stop plus airplane-mode cold
start. A consented intermediate-build recording reached editable review without sending but failed
quality. On 2026-07-19 the final diagnostics-bearing APK `1.0.2+3`
(`1c68fad452bd0525f21c50aeb389825e51a9893126c6c94679ea8004401c3407`) installed in place and the
device `base.apk` matched that host hash. One consented exact-reference recording then reached
editable review with healthy aggregate signal diagnostics and no send. This is install/review
evidence, not the still-open synthetic A/B, lifecycle/resource/thermal, routing, or full
real-speaker matrix.

The self-hosted selector and server-authority cancellation fences are packaged in the later signed
arm64 release `1.0.3+4` (52,300,150 bytes, SHA-256
`b9cede23fc918c2d1a76c3ce3ef5f72a3a1680716ef0c6b6b58c038997f56079`, the same retained release
certificate). That artifact passed host inspection with no bundled model/audio. It has not yet been
installed: the post-build ADB check returned no connected device, so it carries no physical canary
claim.

The standalone device A/B harness compares the production streaming contract with the pinned
offline Paraformer candidate without changing production app storage or authority:

```sh
ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh all streaming-production
# Tap "Run benchmark" on the device, then independently verify the report:
ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh verify-result streaming-production \
  >target/streaming-paraformer-android.json

ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh all offline-paraformer-candidate
# Tap "Run benchmark" again, then independently verify the report:
ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh verify-result offline-paraformer-candidate \
  >target/offline-paraformer-android.json

nix develop --command tools/android_asr_benchmark/benchmark.sh verify-pair \
  target/streaming-paraformer-android.json \
  target/offline-paraformer-android.json \
  >target/android-asr-ab.json
```

The harness requires a real Android process and marks host results ineligible. Its APK has no
microphone, network, session, or send path. See
[`tools/android_asr_benchmark/README.md`](../tools/android_asr_benchmark/README.md) and the
[P6.6 resumption evidence](evidence/2026-07-18-p6-6-on-device-asr-resumption.md).

```sh
nix develop --command bash -lc \
  'cd clients/miku_flutter && flutter build apk --debug'
```

The debug APK is written to `clients/miku_flutter/build/app/outputs/flutter-apk/app-debug.apk`.
If `adb` is not on `PATH`, use the SDK copy directly:

```sh
~/Library/Android/sdk/platform-tools/adb devices
```

Release builds never fall back to the debug key. Either create the untracked
`clients/miku_flutter/android/key.properties` with `storeFile`, `storePassword`, `keyAlias`, and
`keyPassword`, or provide all four `TM_ANDROID_RELEASE_STORE_FILE`,
`TM_ANDROID_RELEASE_STORE_PASSWORD`, `TM_ANDROID_RELEASE_KEY_ALIAS`, and
`TM_ANDROID_RELEASE_KEY_PASSWORD` environment variables from a local secret manager. Then build and
run the repository's authoritative release verifier:

```sh
nix develop --command bash -lc \
  'cd clients/miku_flutter && flutter build apk --release --split-per-abi --target-platform android-arm64'
nix develop --command tools/verify-p6-6-release-apk.sh
```

`tools/verify-p6-6-release-apk.sh` is the authoritative P6.6 package check. By default it verifies
the final split `app-arm64-v8a-release.apk`: application id and the pubspec version (including
Flutter's `+2000` arm64 split version-code offset), non-debuggable and backup/cleartext policy,
exact permissions, arm64-only native payload, the selected sherpa-onnx runtime, absence of bundled
model/audio files, the Android 24+-compatible v2 signature, retained release certificate, byte
count, and SHA-256. Its printed SHA-256 is the independently supplied build identity for the
real-speaker scorer; a manual `apksigner` inspection is supplementary and does not replace this
script.

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
Unknown fields at the host, linked-folder, approval, self-evolution, egress, and isolation policy
boundaries are rejected. A misspelled hardening key therefore fails startup instead of silently
selecting a default-disabled authority profile.

Minimal repo-linked config:

```json
{
  "linked_folders": [
    {
      "name": "repo",
      "path": ".",
      "mode": "rw",
      "commands": ["cargo"]
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
  "proc_isolation": { "provider": "disabled" },
  "artifact_root": ".tempestmiku/artifacts"
}
```

Notes:

- `path` must resolve to an existing directory.
- `mode` is `ro` or `rw`.
- `commands` is an allowlist of executable names for `proc.run`.
- `safe_args` remains accepted for config compatibility but never bypasses approval. Every command
  requires approval, including when the optional Linux isolation profile is enabled.
- Child processes receive only a small explicit environment allowlist for tool discovery,
  temporary files, locale, Rust/Nix toolchains, and macOS SDK selection. Arbitrary server
  environment variables are not inherited. Empty and relative `PATH` entries are dropped; the
  absolute executable path and filesystem identity are resolved before approval and rechecked after
  it. On Unix the child re-stats that device/inode in an allocation-free final `pre_exec` hook.
  Path-based exec still has a narrow stat-to-exec race on platforms without fd-based exec, so this
  check does not replace manual approval or OS isolation. Approval actions are bounded redacted JSON
  with an exact argv digest and descriptor-pinned cwd.
- `proc.run.stdin` accepts an optional UTF-8 string capped at 1 MiB. It shares the command timeout
  with process execution and output collection; non-string or oversized input fails before spawn.
  The approval binds stdin presence, raw byte count and SHA-256, plus a redacted preview capped at
  256 bytes with an explicit truncation marker.
- `proc_run_timeout_ms` sets both the default and maximum per-command timeout. It defaults to
  180000 and must stay between 1 and 900000; benchmark adapters may opt into a larger bounded value.
- On Unix, each `proc.run` command owns a fresh process group and enters its linked cwd through a
  no-follow directory descriptor. Timeout or turn cancellation kills that group and ordinary
  descendants. A process that deliberately calls `setsid(2)` can escape portable group containment;
  every `proc.run` therefore remains approval-gated.
- Linked roots pin their device/inode identity; replacing the configured path with another real
  directory fails until it is explicitly relinked. Reads, recursive list/find/search, and mutation
  commits use descriptor-relative no-follow traversal on Unix and fail closed elsewhere. Mutation
  reads are byte-bounded; recursive walks reject trees deeper than 128 directory levels and stop
  after 100,000 visited entries. List/find/search results are capped at 4 MiB using exact serialized
  JSON accounting, including escaped content and array framing. Policy revision, tag, and
  device/inode identity are rechecked after approval and before commit. A shared policy gate orders
  reads, final mutation
  syscalls, and process validation/spawn against policy replacement/removal: either revocation lands
  first and the old revision fails, or the already-validated operation finishes before revocation
  returns. POSIX exposes no portable inode-conditional rename/unlink, so an unrelated host process
  can still race the final check, although held parent descriptors keep the operation inside the
  linked root.
- Approval mode `deny` is the default. Approval mode `manual` asks before overwrites, removals,
  and every process command.
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

On a reviewed Linux host, `proc_isolation` can opt into the required bubblewrap profile:

```json
{
  "proc_isolation": {
    "provider": "linux_bubblewrap",
    "launcher": "/opt/tempestmiku-isolation-runtime/bin/bwrap",
    "runtime_roots": ["/opt/tempestmiku-isolation-runtime"],
    "limits": {
      "address_space_bytes": 2147483648,
      "process_count": 128,
      "open_files": 1024
    }
  }
}
```

The launcher and every explicit runtime root must be absolute, root-owned, and not group/world
writable; broad or host-sensitive roots are rejected. The profile mounts only the descriptor-pinned
linked root plus those read-only runtime roots, unshares user/mount/PID/IPC/UTS/network namespaces,
drops capabilities, clears ambient environment, and applies bounded `RLIMIT_AS`, `RLIMIT_NPROC`, and
`RLIMIT_NOFILE`. If the launcher/profile disappears or changes while approval is pending, execution
fails before host spawn and never falls back to the disabled path. It is optional and disabled by
default. See the [Linux isolation canary](evidence/2026-07-18-m4-linux-proc-isolation.md).

For the repo-owned fixed seccomp policy plus cgroup-v2 enforcement, opt into the stronger profile
only after the service manager has delegated an exclusive cgroup-v2 subtree with `cpu`, `memory`,
`pids`, and `cgroup.kill` support:

```json
{
  "proc_isolation": {
    "provider": "linux_hardened_v1",
    "launcher": "/opt/tempestmiku-isolation-runtime/bin/bwrap",
    "runtime_roots": ["/opt/tempestmiku-isolation-runtime"],
    "limits": {
      "address_space_bytes": 2147483648,
      "process_count": 128,
      "open_files": 1024
    },
    "cgroup_root": "/sys/fs/cgroup/tempestmiku",
    "cgroup_limits": {
      "memory_max_bytes": 2147483648,
      "memory_swap_max_bytes": 0,
      "pids_max": 128,
      "cpu_quota_micros": 100000,
      "cpu_period_micros": 100000
    }
  }
}
```

`linux_hardened_v1` fails before approval if the policy, launcher/runtime roots, or delegated subtree
cannot be pinned and verified; it never falls back to the lower profile or direct execution. Every
run gets its own cgroup leaf and cleanup kills and drains that leaf after success, timeout,
cancellation, or drop. `tm-server` invokes `ProcIsolationConfig::recover_orphans_at_startup`
automatically, before constructing either its API or worker runtime, and fails startup if recovery
cannot prove and drain the configured subtree. A supervisor must assign a different exclusive
delegated root to every concurrently running `tm-server` instance; sharing one root would let one
instance's startup recovery terminate another instance's work. The disposable Linux/aarch64 canary
passes, but its limits are examples rather than production sizing. M4 still needs a canary under the
chosen production service identity and architecture plus measured workload sizing. It covers a
hostile workload only while trusting the host kernel; if the host kernel is in scope, a
separate-kernel microVM and its production canary are mandatory. See the [hardened Linux
evidence](evidence/2026-07-18-m4-linux-hardened-v1.md).

### M4 production acceptance contract

The checked-in acceptance kit and disposable software canary are not a deployment claim or a
production pass. `linux_hardened_v1` has one precise threat boundary: it contains a hostile workload
while trusting the host kernel. If the host kernel itself is in the threat model, a microVM with a
separate kernel is mandatory; this profile, its container namespace, and the M4 acceptance report do
not claim that assurance.

Start from [`tools/m4-deployment-contract.example.json`](../tools/m4-deployment-contract.example.json)
and its strict [v1 schema](../tools/m4-deployment-contract.schema.json). Replace every example path,
identity, UID, GID, architecture, runtime artifact identity, and service-manager detail with the
selected deployment's final values. Validate it portably before touching the target:

```sh
python3 tools/m4_acceptance.py validate-contract /path/to/m4-deployment-contract.json
```

Before enabling this profile, record and verify all of the following on the selected target:

1. Declare a trusted host kernel plus hostile workload in the contract. If a hostile host kernel is
   in scope, stop: select a mandatory microVM and collect its separate evidence before making that
   claim. A container namespace is not a microVM and does not provide a separate kernel.
2. Give each concurrently running `tm-server` process its own empty cgroup-v2 subtree. The service
   process itself must remain outside that subtree; only its `proc.run` children enter per-run
   leaves. Enable and delegate `cpu`, `memory`, and `pids`, and retain `cgroup.kill`. Do not share a
   root between `api`, `worker`, or `all` instances.
3. Keep the final deployment contract, `TM_HOST_CONFIG`, service-manager config, `tm-server`
   binary, bubblewrap launcher, every runtime root, and their ancestry root-owned and non-writable
   by the service identity. File inputs must be regular non-symlink files. Mount only explicitly
   linked project roots; do not expose an ambient home directory or the whole cgroup filesystem.
4. Choose cgroup and rlimit values from a measured representative coding workload. The example
   2-GiB/128-pid/one-CPU values are test inputs, not production sizing.
5. Start `tm-server` once with the final config and verify the structured startup recovery record.
   Any config, controller, identity, probe, or orphan-cleanup error must abort startup.
6. Under the exact final service UID/GID, architecture, source revision, image/runtime roots,
   linked mount, cgroup namespace, and kernel, run the wrapper from the retained source checkout.
   The UID/GID/architecture below are operator-authored literals copied from the reviewed service
   contract. Do not populate them with `id`, `uname`, command substitution, or discovery inside the
   canary process:

   ```sh
   TM_M4_EXPECTED_UID=10001 \
   TM_M4_EXPECTED_GID=10001 \
   TM_M4_EXPECTED_ARCH=x86_64 \
   TM_HOST_CONFIG=/etc/tempestmiku/host.json \
   TM_M4_DEPLOYMENT_CONTRACT=/etc/tempestmiku/m4-deployment-contract.json \
   TM_M4_EVIDENCE_OUTPUT=/var/lib/tempestmiku/evidence/m4-acceptance.json \
     tools/m4-linux-hardened-canary.sh
   ```

   Preflight requires the service process to be outside the empty, service-owned, exclusive
   delegated root; verifies controller delegation, `cgroup.kill`, linked-root access, all trusted
   identities and modes; and makes the exact Rust canary parse `TM_HOST_CONFIG` through
   `P0HostConfig`, consume its rlimits/cgroup limits, and read those limits back. It rechecks all
   identities, source digests, and zero children/processes after the run.
   A versioned report is atomically created only after success and records the contract/config/
   runtime hashes, Git dirty state, source hashes, cgroup namespace/controllers/limits, assertions,
   captured exact-test output, and explicit claim boundary. Refusal or test failure leaves no
   report. Validate a retained successful report with:

   ```sh
   python3 tools/m4_acceptance.py validate-report /var/lib/tempestmiku/evidence/m4-acceptance.json
   ```

   The report deliberately keeps workload sizing, hostile host-kernel containment, and microVM
   isolation under `scopeBoundary.notProven`. Passing with example limits does not close those
   external choices or measurements.

For the separate native x86_64 architecture gate, manually dispatch
`.github/workflows/m4-native-x86-canary.yml`. It uses a disposable contract and the same exact
wrapper, then uploads only a successful validated JSON report. The workflow definition or an
unexecuted/failed run is not evidence and cannot close production deployment acceptance.

For systemd, controller delegation must be explicit and must yield the empty writable child
described above; merely placing the service in a resource-controlled unit is insufficient. For
OpenRC or a container supervisor, provision and verify the subtree explicitly before dropping to
the service UID. A container-wide `--pids-limit` is useful defense in depth but is not per-run
cgroup delegation. Do not compensate by running the steady-state application privileged or by
mounting the host's entire cgroup tree writable.

### M4 coordinator/worker deployment

Run exactly one authoritative `tm-server` coordinator and one bounded `tm-worker`. The worker has
no model, memory, client API, or independent approval authority. The coordinator's
`TM_SERVER_ROLE=api|worker|all` setting refers only to its internal durable services and is unrelated
to the external `tm-worker` binary.

Use the environment-neutral
[`coordinator/worker deployment guide`](deploy-coordinator-worker.md) for Cargo builds, JSON
configuration, secret-file providers, HTTP/HTTPS rules, arbitrary supervisors, optional Linux
process isolation, rollout, acceptance, and recovery. Startup refuses simultaneous local and remote
linked-folder hosts, and worker loss fails explicitly without local execution fallback.

## Selected MCP and live research

MCP is disabled unless a trusted MCP config explicitly selects both the transport and individual
objects. First add an exact P9 destination and optional opaque secret to the normal host config:

```json
{
  "egress": {
    "enabled": true,
    "destinations": [
      {
        "id": "mcp_docs",
        "scheme": "https",
        "host": "mcp.example.com",
        "port": 443,
        "path_prefixes": ["/rpc"],
        "methods": ["POST"],
        "allowed_request_headers": [
          "accept",
          "content-type",
          "mcp-protocol-version",
          "mcp-session-id"
        ]
      }
    ],
    "secrets": [
      {
        "id": "mcp_docs_token",
        "env": "MCP_DOCS_TOKEN",
        "destinations": ["mcp_docs"],
        "injection": { "kind": "authorization_bearer" }
      }
    ]
  }
}
```

Then create `.tempestmiku/mcp.json` (or set `TM_MCP_CONFIG` to another path):

```json
{
  "enabled": true,
  "servers": [
    {
      "alias": "docs",
      "url": "https://mcp.example.com/rpc",
      "destination_id": "mcp_docs",
      "secret_id": "mcp_docs_token",
      "timeout_ms": 15000,
      "allow": {
        "tools": {
          "lookup": { "mutation": false }
        },
        "resources": ["docs://guide"],
        "prompts": []
      }
    }
  ]
}
```

The names and resource URIs must exactly match the remote catalog; a missing object, collision,
protocol mismatch, malformed schema, or budget violation aborts startup without partially activating
the catalog. The config stores only the environment-variable name; `MCP_DOCS_TOKEN` itself remains
inside the P9 secret broker.

Run either surface with both configs:

```sh
MCP_DOCS_TOKEN='...' \
  TM_CONFIG=.tempestmiku/config.json \
  TM_MCP_CONFIG=.tempestmiku/mcp.json \
  cargo run -p tm-cli -- "Research the selected source and preserve citations."

MCP_DOCS_TOKEN='...' \
  TM_HOST_CONFIG=.tempestmiku/config.json \
  TM_MCP_CONFIG=.tempestmiku/mcp.json \
  cargo run -p tm-server
```

Imported calls remain code APIs discoverable through `tools.search` / `tools.docs`; no new
chat-native tool is added. Remote results are explicit `mcp_untrusted_data` envelopes with bounded
catalog and payload provenance. Locally mapped resources can be listed through
`/sessions/:id/resources/list?uri=mcp://docs` and read by the returned hashed `mcp://` URI. Locally
configured mutations still require manual approval. The catalog is immutable for one process;
restart after changing the trusted config so cached interpreter state cannot retain old bindings.

## E2E Harness

For a scripted public-API run against an already running `tm-server`:

```sh
nix develop --command cargo run -p tm-e2e -- scripted
```

For the full local API evidence suite with an in-process server fixture:

```sh
nix develop --command cargo run -p tm-e2e -- record suite
```

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

For a credentialed live LLM-provider e2e run:

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
- Android release build fails before compilation: provide `android/key.properties` or all four
  `TM_ANDROID_RELEASE_*` values from a local secret manager; release builds intentionally refuse
  debug-signing fallback.
- Code tools say no linked folders are configured: add a host config and set `TM_CONFIG` or
  `TM_HOST_CONFIG`.
- An OpenAI-compatible endpoint rejects the request body: try `OPENAI_STREAM_USAGE=0`.
