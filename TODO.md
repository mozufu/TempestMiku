# TODO

Checked items reflect the implemented/tested runtime as of the 2026-06-30 docs sweep. Unchecked
items are still real contract polish, not necessarily missing behavior.

## JS/TS runtime SDK contract

- [ ] Add an authoritative `tm-runtime.d.ts` artifact/source file from §7.1, or generate it from the Rust capability registry.
- [x] Inject the first-pass JS/TS prelude into each `deno_core` session: `print`, synchronous `display`, `tools`, `resources`, `artifacts`, `fs`, `code`, and `proc`.
- [x] Keep reserved future globals `secrets`, `memory`, `skills`, and `agents` set to `undefined`.
- [ ] Decide the final `http` namespace contract: current M1 exposes default-deny/allowlisted `http.get`; the older reserved-global plan said `http` should be undefined until broader egress policy exists.
- [x] Keep raw ambient APIs unavailable: `Deno.*`, raw `fetch`, raw host process access, raw host filesystem/process/network APIs, raw shell strings, and package installation.
- [x] Route typed namespace wrappers through the dynamic host bridge (`tools.call` / `op_host_call`) instead of adding per-capability Deno ops.
- [x] Implement `tools.search`, `tools.docs`, and `tools.call` with progressive disclosure over the host capability catalog.
- [ ] Enrich `tools.docs(name)` from the capability registry with complete SDK-definition metadata: precise signatures, args/result schemas, executable examples, structured errors, grant requirements, sensitivity, approval policy, and stability.
- [x] Ensure `tools.search(query)` lets the model discover SDK capabilities by natural-language intent before calling `tools.docs(name)`.
- [x] Implement `display(value, opts?)` as a synchronous buffered output primitive and include display items in shaped output.
- [x] Implement `resources.read`, `resources.preview`, and `resources.list` against the §9.2 scheme-dispatched resolver registry.
- [x] Implement `artifacts.put`, `artifacts.get`, `artifacts.slice`, and `artifacts.list` against the session artifact store.
- [x] Implement `fs.read`, `fs.write`, `fs.ls`, and `fs.find` for linked-folder grants.
- [x] Ensure `fs.read` and `resources.read` return `ResourceContent` envelopes, never naked strings.
- [x] Implement `code.search` as regex/text search over granted code paths.
- [x] Implement `code.edit` as JSON-hunk patch editing with optimistic concurrency tags and narrow-hunk validation.
- [x] Implement `proc.run(cmd, args, opts?)` using argv-vector execution, linked cwd resolution, allowlisted commands, safe-arg prefix checks, output caps, and artifact spill.
- [x] Reject shell-style process calls such as `proc.run("cargo test")`, command concatenation, pipes, and redirects.
- [ ] Normalize host-call failures into structured JS errors beyond prefixed error strings, including `NotImplementedError`, `QuotaExceededError`, `TimeoutError`, and `OutputTruncatedError` where applicable.
- [x] Fail closed for unknown capability names, unknown resource schemes, path traversal, missing grants, non-allowlisted commands, and unavailable future namespaces.

## Deferred namespaces

- [ ] Harden `http.*` beyond the current deterministic allowlisted `http.get`: byte/request caps, audit logging, redirect policy, and production egress allowlists.
- [ ] Add `secrets.use` only after the secret broker can return opaque, egress-scoped handles without materializing secret values in JS heap, artifacts, or model context.
- [ ] Add `memory.*` only after memory scopes and resource handlers are implemented.
- [ ] Add `skills.*` only after skill persistence, provenance, and safe import/version semantics are defined.
- [ ] Add `agents.*` only after agent artifacts, status, messaging, and resource reads are implemented.
- [ ] Add `code.ast` after the structural search/edit backend exists.
- [ ] Add `code.lsp` after LSP server lifecycle, diagnostics, references, rename, and code-action behavior are wired through grants.

## Acceptance checks

- [x] A cell can call `display("ok")` without `await`, and the display item appears in the shaped result.
- [x] `secrets`, `memory`, `skills`, and `agents` evaluate to `undefined` instead of throwing `ReferenceError`.
- [x] `http.get(...)` is default-deny and only returns deterministic allowlisted responses.
- [x] Direct raw APIs (`Deno`, raw `fetch`, raw filesystem/process APIs) are unavailable inside the runtime.
- [x] `fs.read(...)` returns a `ResourceContent` envelope with `mime`, `kind`, and `content`/`preview` fields.
- [x] `resources.read(...)` returns the same envelope shape as `fs.read(...)`.
- [x] `code.edit(...)` applies a valid JSON hunk and rejects stale tags or malformed/wide hunks.
- [x] `proc.run("cargo", ["test"], { cwd: "tempestmiku:" })` can run when the linked folder grant and allowlist permit it.
- [x] `proc.run("cargo test")` is rejected before process execution.
- [x] A non-allowlisted command fails closed with a structured capability/policy error.
- [x] Large stdout/stderr spills to `artifact://` and returns a bounded preview.
- [x] Unknown `tools.call("missing.capability", {})` fails closed with a structured error.
- [ ] `tools.docs("fs.read")` returns the `fs.read` signature, argument schema, result schema, at least one executable example, grant requirements, and approval policy without exposing a second chat-native tool.
