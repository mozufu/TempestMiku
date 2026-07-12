# Tempest Miku

TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: a
self-hosted, single-user, characterful AI companion built on a streaming code-execution agent
runtime.

Current implementation status: the workspace contains the M0/M1 runtime, P0-P3+ product slices,
the P4 dreaming/scheduler and P5 drive/research mechanisms, and the audit hardening implementation.
Postgres now uses ordered checksummed migrations and backs sessions, turns, approvals/effects,
dream/cron leases, and drive metadata/link tombstones. `tm-server` supports explicit `api`, `worker`,
and `all` roles, durable `202` message turns, one replayable `session_event` SSE envelope, and secure
single-owner device pairing for Web and Android.

The complete verification matrix passes: strict Rust checks, clean-schema Postgres tests, split
API/worker restart recovery, Flutter tests, signed Android APK builds, Playwright pairing/replay,
RustSec, secret-leak checks, and a physical Android release canary over Tailscale Serve HTTPS. The
canary proved in-app QR confirmation, durable chat, cold-start credential/session recovery,
exact-once offline SSE replay, and immediate device revocation. The audit hardening gate is closed
for the documented single-owner deployment. Production exposure must remain loopback-only behind
an HTTPS reverse proxy or Tailscale Serve; Postgres is mandatory outside loopback and for
`worker`/`all` roles. P6 now includes encrypted provider-neutral push registration/outbox and
authenticated Android notification actions; a production provider and remote killed-process canary
remain open. Broader P6/P7 work such as MCP/trace, cloud drive sync, live egress, and self-evolution
remains out of scope.

## Project docs

- [Running Miku](docs/running-miku.md)
- [Design docs](docs/README.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)
- [Commit message spec](docs/commit-messages.md)
- [LLM-to-Miku E2E hatch](apps/tm-e2e/README.md)
