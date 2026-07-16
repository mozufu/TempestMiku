# Tempest Miku

TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: a
self-hosted, single-user, characterful AI companion built on a streaming code-execution agent
runtime.

Current implementation status: M0/M1 and P0-P5 are closed; Android P6.1-P6.5 are closed while P6.6
voice capture remains explicitly deferred. P7.0-P7.2a are closed, and P8 fuller memory is closed with
self-hosted hybrid lexical/dense recall, scoped typed records, inspectable evidence/corrections,
exact retry/replay context, and safe lexical fallback. Postgres uses ordered checksummed migrations
and backs sessions, turns, approvals/effects, dream/cron leases, drive metadata/link tombstones, and
the durable memory/embedding spine. `tm-server` supports explicit `api`, `worker`, and `all` roles,
durable `202` message turns, one replayable `session_event` SSE envelope, and secure single-owner
device pairing for Web and Android.

The complete verification matrix passes: strict Rust checks, clean-schema Postgres tests, split
API/worker restart recovery, Flutter tests, signed Android APK builds, Playwright pairing/replay,
RustSec, secret-leak checks, and a physical Android release canary over Tailscale Serve HTTPS. The
canary proved in-app QR confirmation, durable chat, cold-start credential/session recovery,
exact-once offline SSE replay, and immediate device revocation. The audit hardening gate is closed
for the documented single-owner deployment. Production exposure must remain loopback-only behind
an HTTPS reverse proxy or Tailscale Serve; Postgres is mandatory outside loopback and for
`worker`/`all` roles. Production UnifiedPush/ntfy delivery, actionable notifications, share/selected
text, and quick capture are closed on physical Android. P7.2b approval-backed persona addenda is the
active product slice; P9 egress/opaque secrets must close before P10 MCP/live research. Cloud sync,
`tm-trace`, AST/LSP, and extra sandbox backends remain demand-triggered.

## Project docs

- [Running Miku](docs/running-miku.md)
- [Design docs](docs/README.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)
- [Commit message spec](docs/commit-messages.md)
- [LLM-to-Miku E2E hatch](apps/tm-e2e/README.md)
