# 7. Language reference — the syntax contract

> Status: experimental / fun. This is the canonical source-level contract for the
> tm design exploration. It does not schedule an implementation or replace the
> shipping TypeScript sandbox.

This reference fixes the source syntax agreed for tm. The execution, approval, replay,
and UI contracts in the preceding sections remain authoritative; this document makes their
surface syntax concrete enough for a parser, checker, benchmark corpus, and future interpreter.

## 7.1 Design center

tm is a small ML-shaped expression language with a data-flow surface:

- a submitted cell is an eager, persistent REPL transaction;
- normal function calls use whitespace application;
- pipelines feed the left value into the final argument of the right application;
- host authority is visible only through the @ capability marker;
- types, capability rows, error sets, and presentation sets are inferred, not written by the
  model;
- braces make nested sequential scope explicit, while a standalone --- separates top-level
  forms in a cell;
- tables use ordinary prelude application, not a separate SQL grammar.

There are no statements, assignment, return, async/await, exceptions, optional chaining,
classes, packages, macros, or user-defined generic types.

## 7.2 Lexical rules

Identifiers start with a Unicode letter or underscore and continue with letters, digits, or
underscores. Lowercase identifiers name values, functions, fields, and capabilities; uppercase
identifiers name constructors and local sum types.

Strings use JSON escapes and support interpolation:

~~~tm
"hello #name"
"found #{length hits} matches"
"literal hash: \#"
~~~

Line comments begin with --. A line containing only optional whitespace, ---, optional
whitespace, and an optional line comment is instead the cell-form separator. The lexer gives
that separator priority over a comment.

Numbers use JSON-shaped integer or decimal syntax. The checker distinguishes integer and decimal
uses where needed; it never coerces a string to a number. The source language has true, false,
and null. In typed tm code null is shorthand for None and requires an Option context; it is
retained only for JSON-boundary fluency.

A URI is an atom with scheme:path or scheme://... syntax:

~~~tm
workspace:src/main.rs
artifact://cell/abc123
https://example.test/api
~~~

The lexer recognizes URI shape; the checker resolves the scheme against the resource/egress
registry and rejects an unknown scheme. A record key is recognized only in record-key position,
so a field such as name: "miku" is not a URI. Quote URI-looking text when it is ordinary data.

## 7.3 Cells, forms, and blocks

~~~ebnf
cell           ::= top_form ("---" top_form)* EOF
top_form       ::= type_decl | let_binding | fun_binding | expr

block          ::= "do" "{" block_form (";" block_form)* "}"
block_form     ::= type_decl | let_binding | expr

let_binding    ::= "let" pattern "=" expr
fun_binding    ::= "fun" lower_name pattern* "=" expr
lambda         ::= "fun" pattern+ "->" expr

type_decl      ::= "type" UpperName "=" "|" variant ("|" variant)*
variant        ::= UpperName [type_payload]
type_payload   ::= type_term | record_schema
record_schema  ::= "{" schema_field ("," schema_field)* "}"
schema_field   ::= lower_name ":" type_term
type_term      ::= builtin_type | UpperName | "List" type_atom | "Option" type_atom
type_atom      ::= builtin_type | UpperName | "(" type_term ")"

expr           ::= if_expr | match_expr | handle_expr | pipe_expr
if_expr        ::= "if" expr "then" expr "else" expr
match_expr     ::= "match" expr "{" match_arm+ "}"
handle_expr    ::= "handle" expr "with" "error" "{" match_arm+ "}"
match_arm      ::= "|" pattern "->" expr
pipe_expr      ::= infix_expr ("|>" application)*
application    ::= postfix_atom+
postfix_atom   ::= atom ("." lower_name)*
atom           ::= literal | lower_name | UpperName | capability | uri | record | list
                  | "(" expr ")" | lambda | block
capability     ::= "@" qualified_name
qualified_name ::= lower_name ("." lower_name)*
literal        ::= string | integer | decimal | "true" | "false" | "null"
uri            ::= scheme ":" uri_tail
record         ::= "{" [record_field ("," record_field)*] "}"
record_field   ::= lower_name | lower_name ":" expr
list           ::= "[" [expr ("," expr)*] "]"

pattern        ::= "_" | lower_name | literal_pattern | constructor_pattern
                  | list_pattern | record_pattern | "(" pattern ")"
constructor_pattern ::= UpperName [pattern]
list_pattern   ::= "[" [pattern ("," pattern)*] "]" | pattern "::" pattern
record_pattern ::= "{" [pattern_field ("," pattern_field)*] ["," "..."] "}"
pattern_field  ::= lower_name | lower_name ":" pattern
~~~

The final form is the cell result. Successful top-level let and fun bindings commit together
after the whole cell completes. --- is syntactic only: it never creates a second cell,
commit point, approval checkpoint, or recovery boundary.

A block is an expression. Its forms execute left to right; let extends to the remaining forms.
It is equivalent to nested let/sequence expressions, not a statement language:

~~~tm
do {
  let before = @fs.read workspace:config.json;
  @code.edit {patch: replace "v1" "v2" before};
  display {kind: "text"} "patched"
}
~~~

Only an explicit par scope may run child expressions concurrently.

Named functions are recursively bound by default. Anonymous functions use the same fun keyword:

~~~tm
fun count_matches lines =
  lines |> filter (fun line -> contains "TODO" line) |> length
~~~

## 7.4 Types and local sum types

Every let, function, lambda, capability call, error set, presentation effect, and capability row
is inferred. Source code has no value/function annotations and no source effect-row syntax.
The checker shows inferred facts in help, typed-cell artifacts, and execution trace:

~~~text
collect : List Path -> List Finding
authority: <fs.read>
errors: <NotFound, ApprovalDenied>
presentation: <display>
~~~

The one intentional exception is a local sum-type schema:

~~~tm
type Finding =
  | Hit {file: String, line: Int, text: String}
  | Ignored {path: Path}
~~~

Type declarations are non-generic. Built-in type constructors such as List and Option may
appear inside a variant schema, but users cannot declare type parameters or generic functions.
The declaration is local to its containing cell or do block. A persistent binding, function
signature, or later cell may not expose a local type; the checker rejects that escape. A display
may project a local value as bounded tagged JSON.

## 7.5 Expressions and precedence

Whitespace application is left associative:

~~~tm
f x y
map (fun x -> x + 1) values
@fs.read workspace:src/lib.rs
~~~

Parentheses group expressions; they are not required for ordinary calls. Field access is direct:

~~~tm
resource.mime
artifact.uri
~~~

If a statically known record lacks a field, checking fails. Accessing a missing field through
dynamic JSON performs a structured MissingField error; it does not return None.

The binding and precedence order, from lowest to highest, is:

| Level | Operators | Associativity |
|---|---|---|
| 1 | |> | left |
| 2 | or | left |
| 3 | and | left |
| 4 | ==, !=, <, <=, >, >= | non-associative |
| 5 | :: | right |
| 6 | +, - | left |
| 7 | *, /, % | left |
| 8 | not, unary - | prefix |
| 9 | application, field access | left |

There is no ! operator. Boolean composition is written with not, and, and or. List construction
and list patterns use right-associative ::; list concatenation remains the pure concat prelude
function.

Pipelines always feed their left result to the final argument of the application on their right:

~~~tm
text |> split "," |> filter predicate
rows |> display {kind: "table", title: "Rows"}
~~~

desugars to:

~~~tm
filter predicate (split "," text)
display {kind: "table", title: "Rows"} rows
~~~

This data-last rule is why presentation options precede the displayed value in direct calls.

## 7.6 Data, records, options, and patterns

tm uses JSON-shaped values: strings, numbers, booleans, null, lists, and records. Record
expressions support field punning but no spread:

~~~tm
let config = {name: "miku", cap: 8}
let view = {name, cap}
let next = merge {cap: 9} config
~~~

The pure merge function is the only record-update surface. In patterns, ... ignores remaining
record fields; it is not expression spread.

Options are built in:

~~~tm
match maybe_path {
  | Some path -> @fs.read path
  | None -> display {kind: "text"} "no path"
}
~~~

Patterns support wildcard, binding, literals, constructor patterns, exact list patterns, list
cons, records, and record rest:

~~~tm
match result {
  | {content, mime: "text/plain", ...} -> content
  | first :: rest -> first
  | [] -> "empty"
  | _ -> "fallback"
}
~~~

There are no pattern guards. Use if in an arm when a second condition is required:

~~~tm
match finding {
  | Hit {line, text} ->
      if line > 100 then text else "early hit"
  | Ignored _ -> "ignored"
}
~~~

if is expression-only and always has an else:

~~~tm
if length hits == 0
then display {kind: "text"} "clean"
else display {kind: "table"} hits
~~~

## 7.7 Tables are ordinary applications

table is a core constructor and table operations are ordinary data-last prelude functions:

~~~tm
let users = table [
  {name: "ice", age: 30, email: "i@x"},
  {name: "miku", age: 21, email: "m@x"}
]
---
users
  |> where (age > 18)
  |> select {name, email}
  |> sort_by name asc
  |> display {kind: "table", title: "Adults"}
~~~

There is no table clause grammar. The parser sees normal application, ordinary records, lists,
and operators. The checker elaborates a known table-prelude argument as a row expression:

- a lexical binding wins when a name is both local and a column;
- otherwise a known schema column such as age is available in that table-expression position;
- row.field is the explicit escape when a local name shadows a column;
- select and aggregate use a record to preserve output field names;
- group_by uses a list of selectors, and sort_by receives selector then asc or desc.

For example:

~~~tm
rows
  |> group_by [file]
  |> aggregate {todos: sum todos, hits: count}
  |> sort_by hits desc
~~~

Heavy analytics, planners, window functions, arbitrary joins, and streaming data remain future
host capabilities rather than syntax growth.

## 7.8 Capabilities, errors, and presentation

@ introduces a registry-owned capability reference:

~~~tm
@fs.read workspace:src/main.rs
@code.edit {patch}
@mcp.github.search_issues {repo: "owner/repo", query: "is:open"}
help @fs.read
~~~

A capability name is static and canonical. Code cannot construct one from a string. @capability
may be passed as an effectful function, for example to par map; applying it is the perform that
is checked against the current cell grants and recorded in the trace.

Registry metadata owns approval policy, resumability, preview redaction, and labels. There is no
approval punctuation in source. A suspendable approved call resumes the same continuation; denial
performs the core error effect.

~~~tm
handle @fs.read workspace:missing.rs with error {
  | NotFound {uri} -> display {kind: "text"} "missing #uri"
  | e -> rethrow e
}
~~~

error is abortive. There is no try/catch or Result wrapper for host success values. rethrow
re-performs an unhandled error so the shaped cell result remains accurate.

display is a core presentation effect, not a capability and not a pure function. print is capped
diagnostic output. Both are inferred and separately recorded from grant-bearing authority.

## 7.9 Structured concurrency

par is a core intrinsic with special evaluation, although its source surface is ordinary
application:

~~~tm
paths |> par map @fs.read

let {source, issue} = par {
  source: @fs.read workspace:src/lib.rs,
  issue: @mcp.github.get_issue {number: 42}
}
~~~

par map fans out a homogeneous collection. par applied to a record evaluates its field
expressions concurrently and preserves their names, allowing heterogeneous outcomes without a
tuple type. Child effects form one structured scope; the first error cancels siblings and is
re-performed by the scope. The event tree exposes the scope and bounded progress to the UI.

## 7.10 Deliberate absences

tm does not add ambient filesystem, network, process, package, or client authority. It has no
dynamic capability lookup, raw shell, reactive re-evaluation, implicit concurrency, runtime
widget construction, source-level approval marker, or general effect-handler syntax. Those
absences are part of the security and fluency contract, not missing convenience features.
