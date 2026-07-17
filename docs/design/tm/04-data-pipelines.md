# 4. Data, pipelines & errors

This section defines the small eager data prelude. The formal grammar and operator precedence
are in [§7](07-language-reference.md); the intent here is to keep the model's common
read-transform-display path compact without turning tm into a dataframe language.

## 4.1 Pipelines are data-last application

The pipeline operator feeds its left value into the final argument of the right application:

~~~tm
workspace:src/main.rs
  |> @fs.read
  |> lines
  |> filter (contains "TODO")
  |> map (replace "TODO" "DONE")
  |> take 10
  |> display {kind: "text", title: "first TODOs"}
~~~

The direct equivalent is:

~~~tm
display {kind: "text", title: "first TODOs"}
  (take 10
    (map (replace "TODO" "DONE")
      (filter (contains "TODO")
        (lines (@fs.read workspace:src/main.rs)))))
~~~

Every stage is a normal prelude function, an @ capability function, or an intrinsic such as
par. There are no pipe-clause productions. This keeps the parser small and makes ordinary
application the only composition rule the model has to learn.

## 4.2 Text, lists, and JSON

The pure prelude covers deterministic, eager in-memory work:

| Function | Data-last use | Notes |
|---|---|---|
| lines, words | text |> lines | TextLike input; no hidden filesystem read. |
| split, join | text |> split "," | Literal delimiters; regex stays explicit. |
| trim, lowercase, uppercase | text |> trim | Unicode-aware text helpers. |
| contains, starts_with, ends_with | lines |> filter (contains "TODO") | Predicate arguments precede the subject. |
| replace | text |> replace "old" "new" | Literal replacement. |
| map, flatmap, filter, fold | values |> filter predicate | Eager collection transforms. |
| take, drop, length, distinct | values |> take 20 | Stable, bounded collection helpers. |
| sort, sort_by, range, concat | values |> sort | No query planner or lazy stream. |
| json.parse, json.stringify, json.pretty | text |> json.parse | Parse errors perform core error. |

Regex stays a pure namespace and uses plain strings:

~~~tm
text |> regex.find_all "TODO|FIXME"
text |> regex.replace "TODO" "DONE"
~~~

The engine is deterministic Rust-regex style: Unicode-aware, no lookaround/backreferences, and
bounded by host-configured pattern/work limits.

Lists use JSON-style literals and right-associative cons:

~~~tm
let paths = [workspace:src/a.rs, workspace:src/b.rs];
let with_readme = workspace:README.md :: paths;
with_readme |> par map @fs.read
~~~

## 4.3 Records, direct fields, and patterns

Records are JSON-shaped values:

~~~tm
let resource = @fs.read workspace:README.md;
match resource {
  | {content, mime: "text/plain", ...} ->
      display {kind: "markdown"} content
  | {artifact, ...} ->
      display {kind: "text"} "spilled: #{artifact.uri}"
  | {preview, ...} ->
      display {kind: "text"} preview
}
~~~

Pattern rest is the only ... syntax. Record update is explicit and immutable:

~~~tm
let next_config = merge {mode: "serious"} config
~~~

value.field is direct access, not an Option-producing operation. Missing fields on a known
record are a check-time error; missing fields in dynamic JSON perform MissingField. Use match
when shape or absence is expected.

## 4.4 Tables are normal applications

A table is a first-class list of records plus known columns. table and its transforms remain
ordinary prelude functions:

~~~tm
let rows = table [
  {file: "a.ts", todos: 3, owner: "ice"},
  {file: "b.ts", todos: 7, owner: "miku"},
  {file: "c.ts", todos: 1, owner: "ice"}
];
rows
  |> where (todos > 2)
  |> select {file, owner, todos}
  |> sort_by todos desc
  |> display {kind: "table", title: "todo counts"}
~~~

where, select, sort_by, group_by, aggregate, inner_join, and left_join have ordinary
data-last call shapes. The parser has no SQL subgrammar. The checker recognizes their
row-expression arguments after ordinary parsing:

~~~tm
rows
  |> group_by [owner]
  |> aggregate {todos: sum todos, files: count}
  |> sort_by todos desc
~~~

An ordinary lexical variable wins over a same-named column. row.field is the explicit column
escape. select and aggregate accept a record so source field names and computed expressions stay
visible:

~~~tm
rows |> select {file, score: todos * 10}
~~~

Small equi-joins remain enough for correlation:

~~~tm
issues |> inner_join owners (left.owner_id == right.id)
~~~

Window functions, pivots, streaming execution, and a query planner belong to a future data
capability, never to tm syntax.

## 4.5 URI and path atoms

URI/path literals are data tokens, not strings:

~~~tm
@fs.read workspace:src/main.rs
@resources.read artifact://cell/abc123
@resources.read mcp://github/repos/owner/repo/issues/123
~~~

Colon belongs to the URI token; @ belongs to a capability reference. A URI does not grant
access. The checker validates its scheme through the existing resource/egress policy and rejects
unknown schemes before evaluation.

## 4.6 Errors are effects, not Result wrappers

Successful capability calls return their success value. Failure performs the core error effect:

~~~tm
handle @fs.read workspace:missing.rs with error {
  | NotFound {uri} ->
      display {kind: "text"} "missing #uri"
  | e -> rethrow e
}
~~~

There is no throw, try, catch, or hidden retry. Host errors such as CapabilityDenied,
ApprovalDenied, ApprovalTimeout, NotFound, Timeout, OutputTruncated, MissingField, and ParseError
are structured payloads supplied by the host/checker. rethrow re-performs the unhandled error so
the shaped cell result remains accurate.

## 4.7 Option and JSON null

Absence is Option:

~~~tm
match maybe_path {
  | Some path -> @fs.read path
  | None -> display {kind: "text"} "no path"
}
~~~

Source null exists only for JSON-boundary fluency. In typed tm it elaborates to None and needs an
Option context; capability failure is separate and always performs error. This removes null
chains without pretending that a failed host effect is ordinary absence.

## 4.8 Why this stays small

- One JSON-shaped value language plus table as a core constructor.
- One application rule and one data-last pipeline rule.
- One structured-success control form, match; one boolean sugar, if; one failure handler,
  handle ... with error.
- One small pure prelude for text, lists, JSON, tables, and bounded regex.
- No classes, method dispatch, dataframe planner, optional chaining, implicit retry, or async
  syntax.

The result is deliberately compact enough to teach through help and to benchmark against the
historical TypeScript prelude that preceded tm-lang.
