# 4. Data, pipelines & errors

The "data-oriented, small syntax" pillar. The goal: the model's 90% case (read → transform →
display, §5.4) is one pipeline; the 10% case (branching on structured results) is one match.

## 4.1 Pipelines are the subject

The pipeline operator `|>` is the default way to write `tm`. Left-to-right, data-first:

```
workspace:src/main.ts
  |> @fs.read
  |> lines
  |> filter (includes "TODO")
  |> map (replace "TODO" "DONE")
  |> take 10
  |> display {kind: "text"}
```

Every stage is either a function `(a -> b)`, a host capability perform (`@capability`), or a
**pipe clause** (`where`, `select`, `sort by`, `take`, `drop`, `group by`, `aggregate`,
`inner_join`, `left_join`) that desugars to a function. Pipe clauses only exist because they
read better than nested calls; semantically they are `filter`/`map`/`sortBy`/etc. The `@`
marker makes boundary-crossing stages visible inside the pipeline instead of hiding host
authority behind ordinary function syntax.

### `par map` — concurrent fan-out, one word

```
paths |> par map @fs.read |> flatmap lines
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

## 4.3 The small data prelude

The prelude should cover the boring 90% so the model does not escape to host capabilities for
ordinary row and string work. This is still not pandas: every function below is eager,
in-memory, and has obvious list/string/table semantics.

### Text

| Function | Type sketch | Notes |
|---|---|---|
| `lines` | `String -> List String` | Sugar for `split "\n"` with trailing newline handling. |
| `words` | `String -> List String` | Split on Unicode whitespace. |
| `split` | `String -> String -> List String` | `text |> split ","`; delimiter is literal text, not regex. |
| `join` | `String -> List String -> String` | `parts |> join ", "`; inverse of `split` for simple cases. |
| `trim`, `trim_start`, `trim_end` | `String -> String` | Unicode whitespace. |
| `includes` / `contains` | `String -> String -> Bool` | `contains` is the canonical name; `includes` is accepted for JS fluency. |
| `starts_with`, `ends_with` | `String -> String -> Bool` | Prefix/suffix tests. |
| `replace` | `String -> String -> String -> String` | Literal replace; regex replacement is a separate helper. |
| `slice` | `Int -> Int -> String -> String` | Half-open `[start, end)` by scalar value, not bytes. |
| `lowercase`, `uppercase` | `String -> String` | Locale-insensitive Unicode case folding. |

### Regex

Regex is a pure prelude namespace, not a host capability and not a new literal form. Patterns
are plain strings so JSON remains the only data literal. The engine should use Rust-regex
semantics: Unicode-aware, deterministic, no lookaround/backreferences, and host-configurable
pattern/step caps to protect replay.

| Function | Type sketch | Notes |
|---|---|---|
| `regex.is_match` | `String -> String -> Bool` | `text |> regex.is_match "TODO|FIXME"`. |
| `regex.find` | `String -> String -> Option RegexMatch` | First match with `{text, start, end}`. |
| `regex.find_all` | `String -> String -> List RegexMatch` | All non-overlapping matches. |
| `regex.captures` | `String -> String -> Option Record` | Named captures become record fields; numbered captures use string keys. |
| `regex.replace` | `String -> String -> String -> String` | Pattern, replacement, text; replacement supports `$name`/`$1`. |
| `regex.split` | `String -> String -> List String` | Regex delimiter split; literal delimiter split stays `split`. |

### Lists

| Function | Type sketch | Notes |
|---|---|---|
| `map`, `flatmap`, `filter`, `fold` | standard | Core collection operators. |
| `take`, `drop` | `Int -> List a -> List a` | Also valid pipe clauses. |
| `length` | `List a -> Int` | Also works on strings and tables. |
| `concat` | `List (List a) -> List a` | Flatten one level. |
| `distinct` | `List a -> List a` | Stable first-seen order; values compare by structural equality. |
| `sort` / `sort_by` | `List a -> List a` | `sort by field desc` is pipe syntax over `sort_by`. |
| `range` | `Int -> Int -> List Int` | Half-open `[start, end)`. |

### URI and path literals

URI/path literals are data tokens, not strings, when they start with a known scheme shape:
`scheme:path` or `scheme://...`. This keeps resource-bearing capability calls compact while
still reserving ordinary strings for ordinary text.

```
@fs.read workspace:src/main.rs
@resources.read artifact://cell/abc123
@resources.read mcp://github/repos/owner/repo/issues/123
```

Colon remains the URI separator; `@` remains the host-boundary marker. Without `@`,
`mcp://...` is only a URI value. With `@mcp.github.search_issues`, the code performs an MCP
tool capability. Resource reads and tool calls stay visibly different.

### JSON

`tm` accepts JSON literals directly, but file and resource boundaries still return bytes or
strings. JSON conversion is therefore prelude, not a host capability:

| Function | Type sketch | Notes |
|---|---|---|
| `json.parse` | `String -> Result Json HostError` | Parses into records/lists/strings/numbers/bools/`Option`. |
| `json.stringify` | `Json -> String` | Stable key order for replay-friendly output. |
| `json.pretty` | `Json -> String` | Human-readable stable formatting. |

### Tables and aggregation

Tables remain "list of records plus a known column set." The prelude adds only the operations
agents use constantly in summaries:

```
rows
  |> group by file
  |> aggregate {todos: sum todos, hits: count}
  |> sort by hits desc
  |> display {kind: "table", title: "TODOs by file"}
```

| Pipe clause / function | Semantics |
|---|---|
| `where expr` | Filter rows. |
| `select field, computed: expr` | Project fields or computed columns. |
| `sort by field [asc|desc]` | Stable sort by field or computed key. |
| `group by field[, ...]` | Group rows by structural key. |
| `aggregate {name: count, total: sum field, avg: avg field, lo: min field, hi: max field}` | Collapse each group to one row. |
| `inner_join other on left_key == right_key` | Hash join; enough for correlating two small host results. |
| `left_join other on left_key == right_key` | Preserve left rows with `None` on missing right side. |

`inner_join`/`left_join` deliberately stop at equi-joins. Anything needing arbitrary predicates,
window functions, pivot/unpivot, streaming execution, or query planning is a host capability
(`data.*`), not language growth.

### Runtime shape helpers

`type_of value` returns a small descriptive tag (`"string"`, `"number"`, `"list"`, `"record"`,
`"table"`, `"option"`, `"result"`, ...). It is for debugging and `help` examples, not for
normal control flow; production branching should use `match`.

Date/time parsing, locale-aware formatting, full CSV dialects, statistical functions, and
large-data execution stay behind future host capabilities. The language prelude stays small:
text, list, JSON, table summaries, and simple joins.

## 4.4 Records and pattern matching

Records are JSON objects. Decomposition is by pattern, not by `.` chaining + null checks:

```
match @fs.read path {
  Ok {content, mime: "text/plain"} -> content |> display
  Ok {artifact, ...}              -> display {kind: "text", text: "spilled: #artifact"}
  Err(NotFound {uri})             -> display {kind: "text", text: "missing: #uri"}
  Err(e)                          -> rethrow e
}
```

`{content, mime: "text/plain"}` matches a record with those fields and binds `content`. `...`
ignores the rest. This replaces the TS `d.content?.split("\n") ?? []` maze from §6.5 —
instead of optional-chaining through a union, you `match` the union.

## 4.5 Errors are values, not exceptions

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

## 4.6 No `null` in the language

JSON has `null`; `tm` has `Option`:

```
type Option T = Some T | None

match @fs.find "*.ts" {
  Ok([])   -> display {kind: "text", text: "no files"}
  Ok(head :: rest) -> display {kind: "json", data: head}
  Err(e)   -> rethrow e
}
```

Host capabilities that can return "absent" return `Option`. The JSON `null` the model writes
in a literal is accepted (for fluency) but typed as `Option` at the boundary. The model never
has to write `?? null` chains because the type system says "this is `Option`, match it."

## 4.7 Interpolation

Strings interpolate with `#ident` or `#{expr}`:

```
let n = 3
display {kind: "text", text: "found #n TODOs in #{n + 1} files"}
```

No template literals, no `${}`. `#` is chosen because it is one character and it is not a
JSON delimiter the model already has to escape.

## 4.8 Why this is "data-oriented"

- **One data literal** (JSON) + one extension (`table`).
- **One control flow** for structured results (`match`).
- **One composition** form (`|>`).
- **One small data prelude** — text/list/JSON/table summaries plus simple joins; heavy
  analytics stays behind host capabilities.
- **No OOP** — no classes, no prototypes, no `this`, no method dispatch. Everything is a
  function or a pipe clause.
- **No async syntax** — effects subsume it; `par` is the only concurrency word.

The bet is that this is *enough* for an agent and *small enough* to teach the model via
`tools.docs` without a 200-line system-prompt SDK section. The fluency benchmark (§5) is what
tests the bet.
