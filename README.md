# Tempest Miku

TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: a
self-hosted, single-user, characterful AI companion built on a streaming code-execution agent
runtime.

Current implementation status: the workspace has the M0 streaming skeleton, M1 `deno_core`
sandbox, host/artifact foundation, P0a OMP ACP bridge, native P0 Serious Engineer dogfood slice,
native Deno HTTP approvals, and the P1 project-manager remote-control surface in place with Rust
test coverage. The next product work is P2: the full personal-assistant baseline for Miku voice,
profile/user recall, personal-assistant state capture, negative-state grounding, and bounded
proactivity.

## Project docs

- [Running Miku](docs/running-miku.md)
- [Design docs](docs/README.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)
- [Commit message spec](docs/commit-messages.md)
- [LLM-to-Miku E2E hatch](apps/tm-e2e/README.md)
