# 2. Syntax tour — the fun part

`tm` is designed so the model's 90% case — *read, transform, display* — is one pipeline and
nothing else. The syntax is a deliberate subset of things models already write fluently
(JSON literals, pipelines, record patterns) with one new idea (`!` for approval effects).

## 2.1 Hello, Miku

```
print "hello, Miku"
```

That's it. No `console.log`, no `Deno.core.ops.op_print`. `print` is a core primitive (§7).

## 2.2 The shape of a cell

A cell is a series of bindings and a final expression. The last expression is the result;
`display` is the *intended* output (§5.4 result shaping).

```
let paths = ["src/a.ts", "src/b.ts", "src/c.ts"]

paths |> par map fs.read
      |> flatmap lines
      |> filter (includes "TODO")
      |> take 20
      |> display {kind: "table", title: "first TODO lines"}
```

Compare the §6.5 TS original:

```ts
const docs = await Promise.all(paths.map(p => fs.read(p)));
const hits = docs.flatMap(d => d.content?.split("\n") ?? []).filter(line => line.includes("TODO"));
display(hits.slice(0, 20), { kind: "json", title: "first TODO lines" });
```

Same semantics. ~40% fewer tokens. No `await`/`Promise.all`/`?.`/`??` tangle — fan-out is one
word (`par map`), null-safety is handled by `lines` returning `[]` on missing content rather
than by an operator maze.

## 2.3 Data is JSON (and a table)

There is exactly one data literal: JSON. Records, arrays, strings, numbers, bools, null —
what the model already emits in tool-call arguments.

```
let cfg = {
  name: "miku",
  modes: ["playful", "serious"],
  cap: 8
}
```

The one extension is a **table** first-class type, because agents spend their lives in rows:

```
let users = table [
  {name: "ice", age: 30, email: "i@x"},
  {name: "miku", age: 21, email: "m@x"},
  {name: "ren", age: 17, email: "r@x"}
]

users
  |> where age > 18
  |> select name, email
  |> sort by name
  |> display {kind: "table"}
```

Tables flow into `display {kind: "table"}` for free (§7 `DisplayTable`). No DataFrame, no
pandas — just rows the host already knows how to render.

## 2.4 Functions and effect rows

```
fun add(x, y) : Int := x + y

fun gather_todos(paths) : {FS Read} [String] :=
  paths |> par map fs.read |> flatmap lines |> filter (includes "TODO")
```

The `: {FS Read} [String]` is the **effect row + return type**. `{FS Read}` means "this
function performs the `FS Read` effect." Empty row (or omitted) means pure — memoizable,
replayable for free (§3.6).

```
fun pure_sum(xs) : [Int] := xs |> fold 0 (+)
```

No effect row → the host may cache it; the transcript may skip re-running it on replay.

## 2.5 The `!` — approval in the type

```
fun save(patch) : {Code Edit!} Unit := code.edit {patch}
```

`Code Edit!` — the `!` marks an effect that **can suspend for human approval**. The host's
approval policy decides whether it actually suspends (§3). The model does not write "if
approved then… else…" — it writes the edit, and the language handles the wait.

```
do
  code.edit {patch}
  display "done"
```

If the policy is `always`, execution suspends at `code.edit`, the host prompts the user, and
`display "done"` runs only on approval. On denial, the effect resumes with an
`ApprovalDenied` value and normal error handling takes over (§4). **No callback was written.**

## 2.6 Pattern matching, errors as values

There are no exceptions. Errors are values, matched:

```
match fs.read path {
  Ok(content)  -> content |> display
  Err(NotFound {uri}) -> display {kind: "text", text: "missing: #uri"}
  Err(e) -> rethrow e
}
```

`#uri` is interpolation in a string. `rethrow` re-performs the error effect so the cell's
shaped result includes it (§5.4 error policy). This maps 1:1 to §6.3's "a killed cell returns
a structured error the model can react to" — but the reaction is a `match`, not a `try/catch`
that might silently swallow.

## 2.7 `par` — concurrency is a word

```
let [a, b, c] = par [ fs.read "a", http.get "b", tools.docs "fs.write" ]
```

`par` evaluates a tuple of effectful expressions concurrently and waits for all. Backed by
Tokio on the deno path, host-side fan-out on the CPython path (§6.6 `host.parallel`) — **same
syntax, different executor.** `par map` is `par` over a list.

## 2.8 Discovering capabilities

`tools.search` and `tools.docs` are effects too (they hit the registry), and they are the
language's **introspection entry point** — the `tm` equivalent of reading its own SDK docs:

```
let docs = tools.docs "fs.read"
docs.signature |> display {kind: "json"}
```

This is §7's progressive disclosure, unchanged: nothing about a capability enters context
until the code asks (§3.2). The difference is `docs.signature` comes back as an *effect row
declaration*, not a TS `.d.ts` string — the model learns `tm` syntax by reading `tm` effect
types.

## 2.9 What's deliberately not here

No classes. No prototypes. No `this`. No macros. No `async`/`await` (effects subsume both).
No exceptions (errors are values). No optional chaining operator (pattern match instead). No
npm. No `Promise`. No generics syntax beyond effect rows. The language is meant to be small
enough that the *entire grammar fits in the system prompt's SDK section* if we want — and
certainly small enough that `tools.docs` can teach it.
