---
name: tm-lang-fluency
description: Write compact, valid tm-conformance-v2 cells for the persistent execute(code) runtime. Always load this runtime-owned skill whenever a model can call execute, independent of persona, mode, or configured skill assets.
---
# tm Language Fluency

Write tm, not JavaScript, TypeScript, Python, or shell.

- Never write `await`, `const`, assignment, `return`, exceptions, optional chaining, object spread,
  or JavaScript arrow functions.
- Bind values with `let name = expr`. Separate top-level forms with `;`; one trailing semicolon is
  allowed and preserves the final value.
- Forms inside one `execute` cell run in order. Multiple `execute` calls returned in one model turn
  form an independent batch; if a later call needs an earlier binding, reference that binding
  explicitly so the runtime schedules it after the producer commits. Keep write-then-test workflows in one
  cell, or sequence them with `do { write_effect; test_effect }`.
- Calls use whitespace. Pipelines pass their left value as the final argument. Write lambdas as
  `fun value -> expr`.
- Call host effects with a literal `@`, for example
  `@code.search {pattern: "TODO", paths: ["repo:src"], regex: false}`.
- Records are JSON-shaped but have no spread. Update with `merge {field: value} base`. Write
  semicolon-separated effect sequences as `do { effect; final_value }`.
- Core transformations are data-last: `text |> lines`, `text |> split ","`,
  `rows |> filter (fun row -> predicate)`, `rows |> map (fun row -> value)`,
  `values |> take 3`, `values |> sum`, and `contains "TODO" line`. There is no `reduce`.
- Use only documented functions. Do not guess helpers such as `drop`, `skip`, `reverse`, or
  `dropFirst`; combine documented primitives or call a named host effect.
- Fan out with `values |> par map (fun value -> @capability {value})`.
- Recover effect errors with
  `handle effect_expr with error { | Pattern -> fallback | error -> rethrow error }`.
- Display options-first/data-last:
  `rows |> display {kind: "table", title: "Rows"}`. Only the final form is the cell result; add
  another `;` and return a value after `display` when both are needed.
- Effects named by active instructions are ready to call. Discover only unknown effects or schemas
  with `@tools.search {query: "patch"}` and `help @fs.patch`.
- Keep cells small. Prefer structured records and host effects over generated scripts, large
  escaped strings, or broad reads.

```tm
let hits = @code.search {pattern: "TODO", paths: ["repo:src"], regex: false};
let paths = hits |> map (fun hit -> hit.path);
paths |> display {kind: "table", title: "Matches"};
paths
```
