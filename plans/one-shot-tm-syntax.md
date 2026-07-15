# One-shot syntax brief: tm

Created from the 2026-07-15 syntax-design discussion. This is a design handoff for the
experimental tm language; it does not authorize implementation of a new sandbox backend.

## Alignment brief

- Outcome: a small, model-friendly, parsable source syntax for tm.
- Audience: the model authoring execute(code) cells, reviewers reading execution traces, and a
  future Rust parser/checker/interpreter.
- Success criteria: one unambiguous cell/block delimiter scheme; whitespace application; inferred
  effects; explicit host boundary; direct table/pipeline flow; no syntax that weakens grants,
  approval, replay, or UI provenance.
- Taste direction: ML-shaped core, data-flow surface, concise but not magical.
- Must-haves: persistent eager REPL, @ capability marker, manual approval as runtime state,
  display as presentation effect, par as structured concurrency, JSON-shaped data, bounded
  local ADTs, URI atoms, and static fail-closed capability names.
- Non-goals: replacing TypeScript now; packages; macros; user generic types; raw shell; reactive
  bindings; optional chaining; source-level approval punctuation; a UI/widget language.

## Approved source contract

- A cell is top-level forms separated only by a standalone ---; one cell remains one atomic
  binding commit.
- Nested sequential work uses do { ...; ... }; semicolons do not escape that explicit scope.
- Calls use whitespace application: fun name arg = expr and fun arg -> expr.
- Named functions are implicitly recursive; types, effect rows, errors, and presentation effects
  are inferred and emitted through help/trace rather than written by the model.
- A local non-generic type declaration may contain its payload schema; it is scoped to the
  containing cell/block and may not leak into persistent REPL bindings.
- if condition then a else b is expression sugar. Pattern arms use leading |, support records,
  list cons, constructors, and wildcard, but no guards.
- Records use direct field access and merge patch record update. Missing dynamic fields are
  structured errors; absence is Option with Some/None. Source null is JSON-boundary shorthand for
  None under an Option context.
- Boolean operators are not, and, or. --- wins over -- comment lexing when it is a standalone
  cell separator.
- Pipelines feed the left value to the final argument of their right application. display options
  therefore precede the displayed value.
- Tables are normal prelude applications with type-directed row-expression elaboration, not SQL
  grammar. Lexical bindings win over same-named columns; row.field is the explicit escape.
- URI atoms use scheme:path or scheme://... and unknown schemes fail closed. @ capability names
  are static canonical registry names and cannot be assembled dynamically.
- par map and par { ... } are the only structured-concurrency surface; they produce observable
  scopes and first-error sibling cancellation.

## Documentation target

The canonical formal specification is docs/design/tm/07-language-reference.md. Keep the syntax
tour, effects, data, backend, and REPL/UI sections aligned with it.

## Acceptance checks

- The tm documents contain no source-level effect annotation syntax.
- Every multi-form cell sample uses top-level ---; every nested sequence uses do braces with
  semicolons; calls use whitespace application, match/error arms lead with |, and display options
  precede the displayed value.
- The reference spells out URI, pipeline, local-type, Option, record, table, capability, and par
  semantics.
- Markdown links and fenced examples remain readable, and the docs-only diff passes git diff
  --check.

## Fresh-thread prompt

Use this plan as the tm syntax contract. First read docs/design/tm/07-language-reference.md and
the preceding tm design documents. Preserve tm as an experimental persistent eager REPL with
explicit effect order, static @ capability names, inferred authority/error/presentation facts,
runtime-owned approvals, and the shared execution trace/UI model. Do not begin a tm interpreter
or change the shipping TypeScript sandbox unless the owner explicitly resumes implementation.
