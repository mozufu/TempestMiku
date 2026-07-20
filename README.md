# Tempest Miku

TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: a
self-hosted, single-user, characterful AI companion built on a streaming code-execution agent
runtime.

The Flutter/Web presentation layer runs on the retained typed HTTP/SSE contracts. The checked-in app
provides a companion-first conversation canvas, durable session history, linked-project browsing,
scoped Drive and resource views, reviewed changes, inline approvals, explicit Mode controls,
protected settings, notifications, share/import review, and explicit-send voice/ASR flows.
`tm-server` stays the authoritative API and security boundary.

Current implementation status: M0-M4 and P0-P10 are closed. P6.6 has a production local
voice-draft implementation and an explicit, default-disabled option for
an owner-controlled home ASR service. The two engines never silently fall back to one another, and
both still land in editable review before any send. Exact local-model activation and airplane-mode
cold-start inspection pass. The latest signed `1.0.3+4` APK installed in place with an exact
host/device hash match and retained pairing; its production catalog exposed the configured home-ASR
label/model and explicit selection sent no audio. A same-device synthetic A/B passes for both pinned
local engines, and one consented exact-reference capture reached review with healthy signal
diagnostics and no send. Permission denial, model delete/reinstall, interrupted/corrupt recovery,
restart persistence, backup/external-residue, recording lifecycle/cleanup, healthy and stopped
self-hosted paths, and distinct exact-once current/new sends also pass. A consented production-local
10-item corpus passes mean CER `0.132922`, p90 `0.241379`, and code-switch mean `0.281980`, with no
empty, truncated, or signal-warning item; audio and raw transcript text were not retained.
P7.0-P7.2b, P8 fuller memory,
P9 production egress/opaque secrets, and P10 selected MCP/live research are closed with focused,
replay, real-Postgres, and live read-only evidence. P8 provides self-hosted hybrid lexical/dense recall, scoped typed records,
inspectable evidence/corrections, exact retry/replay context, and safe lexical fallback. Postgres
uses ordered checksummed migrations
and backs sessions, turns, approvals/effects, dream/cron leases, drive metadata/link tombstones, and
the durable memory/embedding spine. `tm-server` supports explicit `api`, `worker`, and `all` roles,
durable `202` message turns, one replayable `session_event` SSE envelope, and secure single-owner
device pairing for future Web and Android clients.

The established P0-P10 audit and current software-integration matrix pass: strict Rust checks,
clean-schema Postgres tests, split API/worker restart recovery, Flutter tests, signed Android APK
builds, Playwright pairing/replay, RustSec, secret-leak checks, and a physical Android release canary
over Tailscale Serve HTTPS. Separate signed-device and consented-speech evidence closes P6.6, while
the selected persistent homolab hostile-workload/trusted-host-kernel contract closes M4 production
acceptance. The Android canary proved in-app QR confirmation, durable chat, cold-start
credential/session recovery,
exact-once offline SSE replay, and immediate device revocation. The audit hardening gate is closed
for the documented single-owner deployment. Production exposure must remain loopback-only behind
an HTTPS reverse proxy or Tailscale Serve; Postgres is mandatory outside loopback and for
`worker`/`all` roles. Production UnifiedPush/ntfy delivery, actionable notifications, share/selected
text, and quick capture are closed on physical Android. M2 includes fail-closed `tools.call`; P9 is
disabled by default and P10 preserves `execute(code)` as the sole model-visible tool. The optional
Linux bubblewrap namespace/rlimit profile and `linux_hardened_v1` fixed-seccomp/per-run-cgroup
profile pass disposable Linux canaries and the selected homolab service. Its accepted threat model
is a hostile workload on a trusted host kernel. If the host kernel is in scope, a separate-kernel
microVM is mandatory and remains unproven rather than being part of the selected contract. Cloud
sync, `tm-trace`, AST/LSP, and extra sandbox backends remain
demand-triggered.

## Project docs

- [Running Miku](docs/running-miku.md)
- [Design docs](docs/README.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)
- [Commit message spec](docs/commit-messages.md)
- [LLM-to-Miku E2E hatch](apps/tm-e2e/README.md)
