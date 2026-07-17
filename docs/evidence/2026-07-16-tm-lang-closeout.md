# tm-lang T0-T7 closeout evidence

> Historical cutover snapshot. The later same-day
> [tm-lang-only runtime hard cut](2026-07-16-tm-lang-only-runtime.md) removed the fallback runtime,
> selectors, and executable comparative runner while preserving this evidence.

Date: **2026-07-16**
Status: **T0-T7 CLOSED; tm-lang is default; Deno remains selectable fallback**

## Closed scope

- `crates/tm-lang` is a V8-free crate with a handwritten Unicode-aware lexer, spanned AST, Pratt
  parser, deterministic diagnostics, fail-closed checker, persistent tree-walking interpreter, and
  explicit effect state machine.
- The frozen source contract and versioned `tm-conformance-v1` accept/reject corpus cover the first
  supported grammar. Unsupported syntax is rejected rather than guessed.
- Cells implement atomic binding commit and rollback, reset, recursion, patterns, local sums,
  JSON-shaped data, interpolation, tables/pipelines, bounded print/display, structured `par` scopes,
  and wall/step/output/cancellation limits.
- `TmSandbox` derives granted effect signatures from the existing `HostRegistry`, dispatches the
  existing `HostFn` and `ApprovalPolicy`, preserves exact/wildcard grant behavior, adapts the current
  `ResourceRegistry` without widening scheme authority, and exposes registry-backed `@tools.search`
  and `@tools.docs` with tm effect declarations plus approval/resumability metadata.
- The effect machine binds resume tokens to cell, node, origin, and generation; terminal, duplicate,
  stale, and foreign resumes fail closed. Structured effect/scope/display events are bounded,
  sensitive argument previews are redacted, and the existing server sink persists them in replay
  order.
- CLI and server sessions select tm-lang by default. CLI `--deno-sandbox` and server
  `TM_SANDBOX_BACKEND=deno` select Deno, which uses the same refactored host preparation path
  without changing its behavior. `--tm-sandbox` remains an explicit/default-compatible selector.
- The Serious Engineer coding path now selects either Deno or tm-lang behind the same
  `CodingBackend`, worker/session cache, approval broker, agent loop, linked folders, and durable
  event sink. Public HTTP/SSE tests cover approve, deny, default-deny timeout, cancellation,
  runtime eviction/reset, ungranted-effect rejection, exact-once edit, atomic commit/rollback,
  sensitive preview redaction, and cursor replay.
- An isolated PostgreSQL 16 test runs a real tm cell through the public message and approval routes,
  removes a linked-repo file through approval-gated `fs.remove`, reconnects with a new store, and
  replays the suspended/resolved/resumed/result/display/commit/final trace from a cursor.

## Local verification

| Gate | Result |
|---|---|
| `cargo test -p tm-lang` | Pass: parser/checker/effect-machine, frozen corpus, runtime, real registry/resource/approval, rollback, limits, redaction, wildcard and introspection tests. |
| `cargo tree -p tm-lang -e normal` filtered for `deno_core` / `v8` | Empty: the normal tm-lang dependency tree is V8-free. |
| `cargo test` | Pass: workspace unit/integration/doc tests, including the 39-test Deno fallback, CLI fallback selection, tm-e2e workflows, and 251 server tests. |
| Focused public server tm seam | Pass: real `par -> @fs.remove -> display` plus approve/deny/timeout/cancel/reset, exact-once, rollback, redaction, authority denial, and replay coverage. |
| `cargo test -p tm-e2e native_tm_http_sse_e2e_approves_and_replays_structured_trace --test workflow` | Pass: real localhost HTTP/SSE, approval route, live tail, reconnect cursor, ordered trace, and linked-file effect. |
| Isolated PostgreSQL 16 `gated_postgres_native_tm_cell_approves_and_replays_after_store_reconnect` | Pass with `TM_POSTGRES_TESTS=1` and a real temporary cluster; the test does not skip and a new connection replays the durable trace. |
| Full gated PostgreSQL 16 + pgvector matrix | Pass: 26/26, serialized against a clean temporary cluster, including tm runtime restart/replay and P8 dense/lexical gates. |
| `cargo test -p tm-e2e` | Pass: deterministic public-API workflows and both-backend fluency corpus/runtime coverage. |
| `nix develop --command flutter test` | Pass: 67/67 Flutter tests. |
| `clients/miku_web` `npm test` | Pass: authenticated Playwright chat/SSE smoke; one physical-device evidence test remains intentionally skipped. |
| `cargo fmt --all --check` | Pass. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass. |
| `cargo check --workspace --all-targets --all-features` | Pass. |

## Comparative fluency gate

`apps/tm-e2e/fixtures/tm-fluency-v1.json` freezes exactly 20 prompts across read/filter,
edit, fan-out read, approval write, error recovery, and table summary. `tm-e2e fluency` interleaves
TypeScript and tm runs against the same model, executes every generated cell in the corresponding
real sandbox with deterministic hidden fixtures and effect-count/result/display oracles, permits one
recorded repair attempt, writes each raw run immediately to JSONL, and computes first-try success,
backend-neutral generated-code tokens, completion usage, and retry rates. The summary fails closed
unless it contains exactly 20 x 50 x 2 records and all §5.3 thresholds pass. Both model-facing
contracts were calibrated symmetrically, then frozen as `tm-fluency-prompt-v2` before this formal
run.

The formal run used model `gpt-5.5`, four-way bounded concurrency, and one permitted repair attempt.
It produced exactly 2,000 unique `(caseId, backend, run)` records with complete provider usage:

| Metric | TypeScript | tm |
|---|---:|---:|
| Runs | 1,000 | 1,000 |
| First-try task successes | 1,000 (100%) | 1,000 (100%) |
| Eventual task successes | 1,000 | 1,000 |
| Retry runs | 0 (0%) | 0 (0%) |
| Mean first-attempt generated-code tokens | 50.849 | 40.228 |
| Mean first-attempt provider completion tokens | 71.577 | 62.521 |

`protocolComplete`, `usageComplete`, `taskSuccessNotWorse`, `generatedCodeShorter`, and
`retriesWithin12x` are all true in
[`summary.json`](2026-07-16-tm-fluency-prompt-v2/summary.json). The 2,000 generation, execution,
oracle, retry, token, and usage records are retained in
[`raw.jsonl`](2026-07-16-tm-fluency-prompt-v2/raw.jsonl). This evidence earned default status;
Deno remains the tested explicit fallback.
