# tm-lang-only runtime hard cut

Date: 2026-07-16
Status: **CLOSED**

## Decision

`tm-conformance-v1` is the only language accepted by `execute(code)` and `crates/tm-lang` is the
only in-process `Sandbox` implementation shipped by CLI/server. OMP ACP remains a replaceable
external coding bridge, not a second `execute` language.

The cut deliberately has no compatibility alias or runtime selector. Existing durable events are
not migrated or relabeled; newly emitted native coding events use backend label `native-tm`.

## Removed surface

- deleted `crates/tm-sandbox`, including the Deno/V8 runtime and shipped `StubSandbox`;
- removed `deno_core`, `deno_ast`, `deno_error`, V8/serde-v8, and `tm-sandbox` workspace dependencies;
- removed `ExecutionLanguage`, language-specific tool selection, server backend enums, CLI backend
  flags, and the server environment selector;
- removed the checked-in JavaScript declaration artifact `docs/sdk/tm-runtime.d.ts`;
- removed the executable TypeScript-versus-tm fluency command/module and its runtime dependency.

The historical `tm-fluency-prompt-v2` raw evidence remains immutable. Its frozen prompt corpus was
moved beside that evidence at `docs/evidence/2026-07-16-tm-fluency-prompt-v2/corpus.json`; a new
single-language benchmark is outside this slice.

## Preserved behavior

- `tm-lang` now owns artifact/resource/host/linked-folder/drive setup that formerly lived in the
  removed backend crate.
- CLI, normal server chat, native Serious Engineer/Handoff coding, and child actors all construct
  the same `TmSandboxOptions` and exact per-turn grants.
- Approval suspend/resume/deny/timeout/cancel, atomic binding commit, runtime reset, artifact spill,
  linked-repo operations, actors, drive, replay, and client event shapes retain their existing
  boundaries.
- `research.drive` moved from a JavaScript prelude helper to a bounded Rust host capability. It
  preserves local corpus/digest/citation and budget fields; the tm-only implementation reports zero
  agent documents, while explicit actor fan-out remains composable with `agents.*`.

## Verification

The final local gate matrix for this cut passed:

| Gate | Result |
|---|---|
| `cargo check --workspace --all-targets` | Pass |
| `cargo test` | Pass — workspace unit, integration, e2e workflow, and doc tests |
| `cargo fmt --all --check` | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| manifest/lock/source audit for `tm-sandbox`, Deno/V8 crates, runtime selectors, and retired files | Pass — no shipped path remains |

Focused acceptance also covers the native tm HTTP approval/replay suite, tm-e2e actor coordination,
drive/research smoke, CLI linked-repo edit/run/artifact/denial, and the parser regression for handled
record-argument effects.

## Roadmap state

This is a runtime simplification after the closed T0-T7 cutover, not a new product milestone. P7.2b
approval-backed persona addenda remains the next active milestone. P6.6 remains deferred and the
P9-before-P10 boundary is unchanged.
