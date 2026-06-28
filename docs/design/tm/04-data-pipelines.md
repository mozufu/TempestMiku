# 4. Data, pipelines & errors

The "data-oriented, small syntax" pillar. The goal: the model's 90% case (read → transform →
display, §5.4) is one pipeline; the 10% case (branching on structured results) is one match.

## 4.1 Pipelines are the subject

The pipeline operator `|>` is the default way to write `tm`. Left-to-right, data-first:

```
"src/main.ts"
  |> fs.read
  |> lines
  |> filter (includes "TODO")
  |> map (replace "TODO" "DONE")
  |> take 10
  |> display {kind: "text"}
```

Every stage is either a function `(a -> b)` or a **pipe clause** (`where`, `select`, `sort
by`, `take`, `drop`, `group by`) that desugars to a function. Pipe clauses only exist because
they read better than nested calls; semantically they are `filter`/`map`/`sortBy`/etc.

### `par map` — concurrent fan-out, one word

```
paths |> par map fs.read |> flatmap lines
```

Backed by Tokio on the Rust/deno path; by host-side `host.parallel` (§6.6) on the CPython
path. The model does not write `Promise.all` or `await asyncio.gather` — it writes `par map`
and the backend decides how to fan out. This is the §6.5 fan-out pattern with the ceremony
removed.

## 4.2 Tables

A `table` is a first-class type: a list of records plus a known column set. It exists because
agents do a *lot* of row processing and `display {kind: "table"}` (§7) is already a
first-class output kind.

```
let rows = table [
  {file: "a.ts", todos: 3},
  {file: "b.ts", todos: 7},
  {file: "c.ts", todos: 1}
]

rows
  |> where todos > 2
  |> sort by todos desc
  |> select file, todos
  |> display {kind: "table", title: "todo counts"}
```

Tables are not a dataframe library. No lazy eval, no SQL planner — `where`/`select`/`sort by`
desugar to `filter`/`map`/`sortBy` over a `List Record`. The point is **readability and a
clean `display` path**, not analytics horsepower. If the model needs real analytics it calls a
host capability (e.g. a future `data.*` namespace); the language stays small.

## 4.3 Records and pattern matching

Records are JSON objects. Decomposition is by pattern, not by `.` chaining + null checks:

```
match fs.read path {
  Ok {content, mime: "text/plain"} -> content |> display
  Ok {artifact, ...}              -> display {kind: "text", text: "spilled: #artifact"}
  Err(NotFound {uri})             -> display {kind: "text", text: "missing: #uri"}
  Err(e)                          -> rethrow e
}
```

`{content, mime: "text/plain"}` matches a record with those fields and binds `content`. `...`
ignores the rest. This replaces the TS `d.content?.split("\n") ?? []` maze from §6.5 —
instead of optional-chaining through a union, you `match` the union.

## 4.4 Errors are values, not exceptions

There is no `throw` / `try` / `catch`. Errors are variants of `Result`, and the host's
structured errors (§7 `HostError` — `CapabilityDeniedError`, `ApprovalDeniedError`,
`TimeoutError`, `OutputTruncatedError`, …) are constructors of an `Err` type:

```
type Result T E = Ok T | Err E

type HostError =
  | CapabilityDenied {capability: String}
  | ApprovalDenied {capability: String}
  | ApprovalTimeout {capability: String}
  | NotFound {uri: String}
  | NotImplemented {name: String}
  | Timeout {cell: String}
  | OutputTruncated {bytes: Int}
  | HostCall {detail: Json}
```

This is a 1:1 lift of §7's `HostError` interface into a sum type. The model `match`es on it;
the host emits it as JSON across the bridge. **No exception ever crosses the isolate
boundary** — which matches §6.3's "a killed cell returns a structured error the model can
react to," but makes the reaction a type-level `match` instead of a runtime catch that might
swallow.

### `rethrow`

`rethrow e` re-performs the error's effect so the cell result includes it (§5.4 error
shaping). It is the "I don't handle this" arm — the equivalent of `throw` in TS, but it is
just another effect perform, not a control-flow bazooka.

## 4.5 No `null` in the language

JSON has `null`; `tm` has `Option`:

```
type Option T = Some T | None

match fs.find "*.ts" {
  Ok([])   -> display {kind: "text", text: "no files"}
  Ok(head :: rest) -> display {kind: "json", data: head}
  Err(e)   -> rethrow e
}
```

Host capabilities that can return "absent" return `Option`. The JSON `null` the model writes
in a literal is accepted (for fluency) but typed as `Option` at the boundary. The model never
has to write `?? null` chains because the type system says "this is `Option`, match it."

## 4.6 Interpolation

Strings interpolate with `#ident` or `#{expr}`:

```
let n = 3
display {kind: "text", text: "found #n TODOs in #{n + 1} files"}
```

No template literals, no `${}`. `#` is chosen because it is one character and it is not a
JSON delimiter the model already has to escape.

## 4.7 Why this is "data-oriented"

- **One data literal** (JSON) + one extension (`table`).
- **One control flow** for structured results (`match`).
- **One composition** form (`|>`).
- **No OOP** — no classes, no prototypes, no `this`, no method dispatch. Everything is a
  function or a pipe clause.
- **No async syntax** — effects subsume it; `par` is the only concurrency word.

The bet is that this is *enough* for an agent and *small enough* to teach the model via
`tools.docs` without a 200-line system-prompt SDK section. The fluency benchmark (§5) is what
tests the bet.
