# Tempest Miku

TempestMiku is a Rust rewrite of the running `hermes-agent` Tempest Miku deployment: a
self-hosted, single-user, characterful AI companion built on a streaming code-execution agent
runtime.

Current implementation status: the workspace has the M0 streaming skeleton, M1 `deno_core`
sandbox, host/artifact foundation, P0a OMP ACP bridge, and native P0 Serious Engineer dogfood
slice in place with Rust test coverage. The next product work is P1 project manager + remote
control, while the runtime SDK contract still needs documentation polish such as an authoritative
`tm-runtime.d.ts`.

## Project docs

- [Design docs](docs/README.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)
- [Commit message spec](docs/commit-messages.md)
