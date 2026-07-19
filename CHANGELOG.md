# Changelog

All notable changes to TempestMiku will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses milestone-based versioning until a release cadence exists.

## [Unreleased]

### Added
- Closed P2 actual-output voice parity with a retained non-echo General/grounding/Serious live
  report; negative-state grounding now makes its one `<=10 minute` action bound explicit.
- Added opt-in `OPENAI_CONNECTION_REUSE=0` compatibility for OpenAI-compatible proxies that time
  out later sequential streamed requests while keeping connection reuse enabled by default.
- Added pinned full-history Gitleaks CI plus a narrow deterministic-test allowlist; final history and
  tracked-plus-untracked worktree scans report no leaks.
- Added M2 late-bound `tools.call` with target-grant rechecks while preserving `execute(code)` as
  the sole model-visible tool.
- Closed P7.2b with bounded P8-backed Auto persona proposals and durable manual-only activation and
  rollback.
- Added default-disabled P9 exact-destination HTTPS egress, opaque destination-scoped secret
  handles, durable cross-instance budgets, mutation intents, revocation, and bounded audits.
- Added P10 selected MCP `2025-11-25` imports with strict catalog/schema/result bounds, untrusted
  provenance envelopes, Streamable HTTP, durable approved mutations, and an official read-only live
  canary through P9.
- Added the optional M4 Linux bubblewrap namespace/rlimit `proc.run` profile and the
  `linux_hardened_v1` fixed-seccomp/per-run-cgroup profile with fail-closed launcher/runtime
  validation, plus a strict deployment-contract/acceptance-kit workflow. The kit alone is not
  production pass evidence; the later selected persistent homolab service closes delegation,
  workload sizing, native x86_64, cgroup exclusivity, persistence, and restart acceptance. The
  accepted profile contains a hostile workload while trusting the host kernel; if the host kernel is
  in scope, a separate-kernel microVM remains mandatory and unproven rather than part of this
  contract.
- Added the resumed P6.6 foreground-only local voice-draft path: verified owner-installed
  sherpa-onnx model, bounded PCM capture/inference, on-device Traditional-Chinese conversion,
  cleanup, editable review, privacy-preserving aggregate signal and installed-build diagnostics,
  and explicit current/new-session send without cloud fallback or auto-send. The fail-closed
  Android A/B verifier now recomputes all quality/runtime gates and binds both engines to one
  physical installation. The signed Android lifecycle/failure/current/new matrix and consented
  10-item production-local speaker corpus subsequently closed P6.6 and full P6.
- Added an explicit, default-disabled home-hosted ASR option beside the local P6.6 engine. The
  authenticated server broker fixes the operator destination, accepts only bounded 16 kHz mono
  PCM16, retains no audio/transcript, and never falls back; Flutter requires a privacy disclosure
  before selection and keeps every result in editable review. The current owner-controlled trial
  uses TEA-ASR 1.1 mini for Taiwan Mandarin.
- Built and verified the signed arm64 `1.0.3+4` client containing that option. Server-target changes
  cancel active voice work, revoke prior remote consent, return selection to local, and epoch-fence
  stale catalogs, dialogs, requests, and transcripts. Flutter analyze, 20 focused voice widgets,
  and the 113-test full suite pass. The later physical install retained pairing, matched the host APK
  hash, and closed the selector, healthy/failure, lifecycle, and send canaries.
- Installed the final `1.0.2+3` diagnostics APK in place and matched the device `base.apk` to the
  host SHA-256. A consented exact-reference capture reached editable review with healthy aggregate
  signal diagnostics, retained pairing, and sent no message; this remains a historical checkpoint
  superseded by the `1.0.3+4` closeout.
- Added approve, deny, and timeout handling through the shared session approval route. This first
  served the now-retired Deno Serious Engineer path and is retained by the native tm-lang backend.
- Completed the P1 project-manager remote-control surface with mode lock/override, project views,
  session promotion, resource gateway flows, and Flutter Web/PWA approval/resume coverage.
- Added the initial Flutter Web/PWA and Android client scaffold under `clients/miku_flutter`.
- Added the P0 OMP ACP coding handoff path with approval/artifact routes, backend event normalization, WebUI approval controls, and live-smoke coverage.
- Added the P0 Serious Engineer dogfood slice with linked-folder `fs.*`, `code.*`, and `proc.*` SDK
  access, shared persona modes, the original CLI Deno sandbox wiring, minimal project memory recall,
  approval-aware host policy, and live-smoke coverage. The Deno backend was later removed by the
  native tm-lang-only hard cut.
- Completed the historical M1 Deno/TypeScript sandbox path with transpilation, host-call ops,
  artifact/resource SDK bridging, default-deny `http.get`, output spill, reset, timeout, and security
  regression coverage. That implementation was later removed; tm-lang is now the sole runtime.

- Added a root `ROADMAP.md` as the canonical roadmap for core and product milestones.
- Added this changelog.
