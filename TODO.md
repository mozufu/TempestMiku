# TODO

## JS/TS runtime SDK contract

- [ ] Add an authoritative `tm-runtime.d.ts` artifact/source file from §7.1, or generate it from the Rust capability registry.
- [ ] Inject the first-pass JS/TS prelude into each `deno_core` session: `print`, synchronous `display`, `tools`, `resources`, `artifacts`, `fs`, `code`, and `proc`.
- [ ] Explicitly set reserved future globals to `undefined`: `http`, `secrets`, `memory`, `skills`, and `agents`.
- [ ] Keep raw ambient APIs unavailable: `Deno.*`, raw `fetch`, browser globals, Node built-ins, raw host filesystem/process/network APIs, raw shell strings, environment variables, and package installation.
- [ ] Route typed namespace wrappers through the dynamic host bridge (`tools.call` / `op_host_call`) instead of adding per-capability Deno ops.
- [ ] Implement `tools.search`, `tools.docs`, and `tools.call` with progressive disclosure over the host capability catalog.
- [ ] Populate `tools.docs(name)` from the capability registry with SDK-definition metadata: signature, args/result schemas, examples, structured errors, grant requirements, sensitivity, approval policy, and stability.
- [ ] Ensure `tools.search(query)` lets the model discover SDK capabilities by natural-language intent before calling `tools.docs(name)`.
- [ ] Implement `display(value, opts?)` as a synchronous buffered output primitive and include display items in result shaping before stdout/return values.
- [ ] Implement `resources.read`, `resources.preview`, and `resources.list` against the §9.2 scheme-dispatched resolver registry.
- [ ] Implement `artifacts.put`, `artifacts.get`, `artifacts.slice`, and `artifacts.list` against the session artifact store.
- [ ] Implement `fs.read`, `fs.write`, `fs.ls`, and `fs.find` for workspace and linked-folder grants.
- [ ] Ensure `fs.read` and `resources.read` return `ResourceContent` envelopes, never naked strings.
- [ ] Implement `code.search` as regex/text search over granted code paths.
- [ ] Implement `code.edit` as JSON-hunk patch editing with optimistic concurrency tags and narrow-hunk validation.
- [ ] Implement `proc.run(cmd, args, opts?)` using argv-vector execution, linked cwd resolution, allowlisted commands, safe-arg prefix checks, output caps, and artifact spill.
- [ ] Reject shell-style process calls such as `proc.run("cargo test")`, command concatenation, pipes, and redirects.
- [ ] Normalize host-call failures into structured errors: `CapabilityDeniedError`, `ApprovalDeniedError`, `ApprovalTimeoutError`, `NotFoundError`, `NotImplementedError`, `InvalidPathError`, `InvalidArgsError`, `QuotaExceededError`, `TimeoutError`, `OutputTruncatedError`, and `HostCallError`.
- [ ] Fail closed for unknown capability names, unknown resource schemes, path traversal, missing grants, non-allowlisted commands, and unavailable future namespaces.

## Deferred namespaces

- [ ] Add `http.*` only after network egress allowlists, byte/request caps, audit logging, and redirect policy exist.
- [ ] Add `secrets.use` only after the secret broker can return opaque, egress-scoped handles without materializing secret values in JS heap, artifacts, or model context.
- [ ] Add `memory.*` only after memory scopes and resource handlers are implemented.
- [ ] Add `skills.*` only after skill persistence, provenance, and safe import/version semantics are defined.
- [ ] Add `agents.*` only after agent artifacts, status, messaging, and resource reads are implemented.
- [ ] Add `code.ast` after the structural search/edit backend exists.
- [ ] Add `code.lsp` after LSP server lifecycle, diagnostics, references, rename, and code-action behavior are wired through grants.

## Acceptance checks

- [ ] A cell can call `display("ok")` without `await`, and the display item appears in the shaped result.
- [ ] `http`, `secrets`, `memory`, `skills`, and `agents` evaluate to `undefined` instead of throwing `ReferenceError`.
- [ ] `http?.get?.("https://example.com")` does not throw and does not perform network I/O.
- [ ] Direct raw APIs (`Deno`, raw `fetch`, raw filesystem/process APIs) are unavailable inside the runtime.
- [ ] `fs.read(...)` returns a `ResourceContent` envelope with `mime`, `kind`, and `content`/`preview` fields.
- [ ] `resources.read(...)` returns the same envelope shape as `fs.read(...)`.
- [ ] `code.edit(...)` applies a valid JSON hunk and rejects stale tags or malformed/wide hunks.
- [ ] `proc.run("cargo", ["test"], { cwd: "tempestmiku:" })` can run when the linked folder grant and allowlist permit it.
- [ ] `proc.run("cargo test")` is rejected before process execution.
- [ ] A non-allowlisted command fails closed with a structured capability/policy error.
- [ ] Large stdout/stderr or resource content spills to `artifact://` and returns a bounded preview.
- [ ] Unknown `tools.call("missing.capability", {})` fails closed with a structured error.
- [ ] `tools.docs("fs.read")` returns the `fs.read` signature, argument schema, result schema, at least one executable example, grant requirements, and approval policy without exposing a second chat-native tool.
