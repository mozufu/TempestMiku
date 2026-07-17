# tm-conformance-v2 semicolon and batch hardening evidence

Date: 2026-07-17

## Outcome

`tm-conformance-v2` is the sole active `execute(code)` source contract. It hard-cuts the standalone
`---` form separator to semicolons and replaces speculative unbound-name retry with a scope-aware
forward dependency graph. The `execute` tool and `Session::eval_batch` wire interfaces did not
change.

The frozen `tm-conformance-v1` fixture directory and 2026-07-16 fluency evidence remain historical
artifacts. No active runtime prompt, tool description, conformance test, or product fixture claims
v1 support.

## Frozen behavior

- `cell ::= top_form (";" top_form)* [";"] EOF`; one trailing semicolon preserves the final value,
  while `;;` fails deterministically.
- A standalone legacy separator fails as `TM1006` before it can become a `--` comment.
- Free reads connect to the nearest earlier top-level `let`/`fun` writer in response order.
- Independent cells evaluate concurrently from the pre-batch snapshot. Each dependent fork
  overlays only its successful producer commits, and the session applies final commits in response
  order.
- Check, approval, runtime, timeout, cancellation, and output failures block dependents with
  `BatchDependencyError`. Blocked cells emit no effect or binding event and never fall back to an
  older binding.
- Backward references and effect ordering without a binding edge remain unsupported.

## Deterministic proof

- The v2 accept/reject corpus covers semicolon cells, trailing separators, legacy diagnostics, the
  frozen language surface, and invalid empty forms.
- Runtime tests cover new binding chains, rebinds of existing names, response-order write
  collisions, closure/interpolation capture, independent host-call overlap, backward references,
  producer failure, unrelated-cell survival, zero dependent effects, and persistent-state merge.
- Core, CLI, server, Postgres-gated test surfaces, and tm-e2e public API fixtures all use the active
  v2 syntax and runtime contract.

Commands completed during closeout:

```text
cargo test -p tm-lang
cargo test
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

The historical TypeScript comparator remains retired. A future opt-in v2-only live replay may
record model-generation fluency without changing the hard-cut runtime contract or normal test
matrix.
