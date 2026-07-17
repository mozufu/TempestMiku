# 2. Syntax tour — the fun part

The complete grammar is in [§7](07-language-reference.md). This tour shows the source surface
the model should normally write: a small ML-shaped core, ordinary whitespace application, and a
data-flow shell around the host boundary.

## 2.1 Hello, Miku

~~~tm
print "hello, Miku"
~~~

print is capped diagnostic output. User-facing output uses display with an explicit presentation
specification:

~~~tm
display {kind: "text"} "hello, Miku"
~~~

## 2.2 A persistent cell

A cell contains top-level forms separated by semicolons. One trailing semicolon is allowed and
preserves the last form as its result. The complete cell evaluates now and commits its successful
top-level bindings together; `;` is not a sub-cell or a second transaction.

~~~tm
let paths = [
  workspace:src/a.rs,
  workspace:src/b.rs,
  workspace:src/c.rs
];
let hits =
  paths
  |> par map @fs.read
  |> flatmap lines
  |> filter (fun line -> contains "TODO" line)
  |> take 20;
display {kind: "table", title: "first TODO lines"} hits
~~~

tm is expression-oriented. A nested sequence is explicit with do braces and semicolons:

~~~tm
do {
  @fs.remove target;
  display {kind: "text"} "done"
}
~~~

There are no reactive definitions. A binding is the value computed by this eager REPL cell,
not a subscription that silently re-runs an effect.

## 2.3 Whitespace application and pipelines

Calls use left-associative whitespace application:

~~~tm
contains "TODO" line
filter predicate values
@fs.read workspace:src/main.rs
~~~

Pipelines put their input in the final argument position:

~~~tm
text |> split "," |> filter predicate
rows |> display {kind: "table", title: "Rows"}
~~~

That means the direct forms are:

~~~tm
filter predicate (split "," text)
display {kind: "table", title: "Rows"} rows
~~~

Ordinary parentheses only group an expression. They are useful for a partially applied
predicate, not required around each call.

## 2.4 JSON-shaped data and local types

Values are JSON-shaped records, lists, strings, numbers, booleans, and JSON-boundary null.
Record punning keeps common values compact; merge is the explicit immutable record update:

~~~tm
let config = {name: "miku", cap: 8};
let next = merge {cap: 9} config;
display {kind: "json"} next
~~~

The checker infers value and effect types. The only source-level type spelling is a local,
non-generic sum-type schema:

~~~tm
do {
  type Finding =
    | Hit {file: String, line: Int, text: String}
    | Ignored {path: Path};
  let finding = Hit {file: "src/lib.rs", line: 42, text: "TODO"};
  match finding {
    | Hit {file, line, text} ->
        display {kind: "text"} "#file:#line #text"
    | Ignored {path} ->
        display {kind: "text"} "ignored #path"
  }
}
~~~

That type is scoped to this cell. It cannot leak into a persistent REPL binding or later cell.
The checker instead emits inferred function, authority, error, and presentation facts through
help and the typed execution trace.

## 2.5 Tables are applications, not SQL grammar

table is a core constructor. where, select, sort_by, group_by, and aggregate are normal
data-last prelude functions:

~~~tm
let users = table [
  {name: "ice", age: 30, email: "i@x"},
  {name: "miku", age: 21, email: "m@x"},
  {name: "ren", age: 17, email: "r@x"}
];
users
  |> where (age > 18)
  |> select {name, email}
  |> sort_by name asc
  |> display {kind: "table", title: "Adults"}
~~~

The parser sees only application and ordinary expressions. In a known table-prelude argument,
the checker resolves an otherwise-unbound name such as age against the table schema. A lexical
binding wins; row.age is the explicit escape when names collide.

## 2.6 The @ host boundary

Every registry-owned capability starts with @:

~~~tm
@fs.read workspace:src/main.rs
@resources.read artifact://cell/abc123
@mcp.github.search_issues {repo: "owner/repo", query: "is:open"}
~~~

The capability name is static and canonical. A URI is only data; @mcp.github.search_issues is a
tool perform. Unknown capability names and URI schemes fail before evaluation. @capability can
also be passed as an effectful function:

~~~tm
paths |> par map @fs.read
help @fs.read
~~~

Applying the capability is the perform that gets checked against current grants and becomes a
provenance node. The source never writes an effect row or approval suffix.

## 2.7 Approval is execution state

~~~tm
fun remove target = @fs.remove target
~~~

The checker infers save's authority and possible errors. Registry metadata decides whether a
particular fs.remove perform suspends. If it does, the isolate waits, the UI emits
effect_suspended, and the same continuation resumes after approval. The source is unchanged if
the session was already approved.

~~~tm
handle do {
  @fs.remove target;
  display {kind: "text"} "removed"
} with error {
  | ApprovalDenied {capability} ->
      display {kind: "text"} "denied #capability"
  | e -> rethrow e
}
~~~

There are no exceptions or Result wrappers around successful host values. A failure performs the
core error effect; handle ... with error makes recovery explicit.

## 2.8 Pattern matching, options, and conditionals

~~~tm
match maybe_path {
  | Some path -> @fs.read path
  | None -> display {kind: "text"} "no path"
}

if length hits == 0
then display {kind: "text"} "clean"
else display {kind: "table"} hits
~~~

null is accepted only as JSON-boundary shorthand for None under an Option context. Direct field
access is strict: a known missing field is a type error and a dynamic missing field performs a
structured error. There is no optional-chaining syntax.

## 2.9 par is visible concurrency

~~~tm
let {source, issue} = par {
  source: @fs.read workspace:src/lib.rs,
  issue: @mcp.github.get_issue {number: 42}
}
~~~

par is the only concurrency word. It creates a structured scope with child effect nodes and
bounded progress. On first failure it cancels siblings and re-performs that failure. The model
never writes Promise.all, await, manual abort plumbing, or a thread API.

## 2.10 What deliberately stays out

No classes, prototypes, this, macros, packages, generic user types, source annotations, async
syntax, exceptions, optional chaining, assignment, raw shell, dynamic capability lookup,
reactive bindings, query planner, or UI widget language. The grammar stays small enough for
progressive disclosure and a fluency benchmark, while the trace carries the operational detail.
