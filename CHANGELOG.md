# Changelog

All notable changes to TempestMiku will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses milestone-based versioning until a release cadence exists.

## [Unreleased]

### Added
- Added native Deno HTTP approvals for the Serious Engineer backend, including approve, deny, and
  timeout handling through the shared session approval route.
- Completed the P1 project-manager remote-control surface with mode lock/override, project views,
  session promotion, resource gateway flows, and Flutter Web/PWA approval/resume coverage.
- Added the initial Flutter Web/PWA and Android client scaffold under `clients/miku_flutter`.
- Added the P0 OMP ACP coding handoff path with approval/artifact routes, backend event normalization, WebUI approval controls, and live-smoke coverage.
- Added the P0 Serious Engineer dogfood slice with linked-folder `fs.*`, `code.*`, and `proc.*` SDK access, shared persona modes, CLI Deno sandbox wiring, minimal project memory recall, approval-aware host policy, and live-smoke coverage.
- Completed the M1 sandbox path with TypeScript transpilation, host-call ops, artifact/resource SDK bridging, default-deny `http.get`, output spill, reset, timeout, and security regression coverage.

- Added a root `ROADMAP.md` as the canonical roadmap for core and product milestones.
- Added this changelog.
