# 07 — Host capabilities and tm standard library

## 7.1 One tool, late-bound capabilities

The chat protocol exposes one model-visible tool: `execute(code)`. Inside tm, external work uses
effect syntax and is resolved by `tm_host::HostRegistry`:

```tm
let docs = @tools.search {query: "drive search"}
---
let spec = @tools.docs {name: "drive.search"}
---
@drive.search {query: "approval", project: "tempestmiku", returnSnippets: true}
```

`tools.search` and `tools.docs` are generated from registry metadata. There is no checked-in
JavaScript/TypeScript declaration surface; the removed `docs/sdk/tm-runtime.d.ts` is not an
authority source. The frozen tm syntax contract lives in §tm/07, while capability signatures,
schemas, approval metadata, errors, and examples live beside each `HostFn`.

## 7.2 Registration is not authority

Each turn replaces `TmSandboxOptions.grants` with the exact mode/session/actor grant set.
Configured linked folders, drive availability, registered handlers, or previous turns never union
authority into the current turn. Unknown capabilities and resource schemes fail closed.

Wildcard grants are namespace bounded (`drive.*`, `agents.*`); exact grants such as
`resources.read:drive` and `research.drive` remain separate. Child actors receive only explicitly
delegated grants.

## 7.3 Shipped namespaces

- `tools.search`, `tools.docs`, `tools.call` — bounded catalog discovery/late-bound invocation.
- `resources.read`, `resources.preview`, `resources.list` — capability-gated access to registered
  URI schemes.
- `artifacts.put`, `artifacts.list` — content-addressed session output and spill references.
- `http.get` — allowlisted HTTP read; P9 owns production DNS/IP/redirect/budget hardening.
- `fs.read/write/list/find`, `code.search/edit`, `proc.run` — curated linked-repo reach.
- `drive.put/get/ls/move/search/tag/link/unlink/organize` — local-first drive operations.
- `research.drive` — bounded deterministic local digests and `drive://` citations. It is a Rust
  host capability after the tm-only cut; `maxWorkers` is retained in the result contract but local
  execution reports zero agent documents.
- `agents.run/spawn/parallel/msg/send/broadcast/wait/inbox/list/cancel/pipeline` — bounded actor
  orchestration when the turn holds `agents.*`.
- `modes.suggest` — approval-backed mode suggestion available only to eligible unlocked turns.

Server-owned memory, managed-skill activation, persona evolution, approval resolution, and secret
material are intentionally not ambient language namespaces. Their readable evidence may be exposed
through capability-gated resources.

## 7.4 Real-repo boundary

Linked folders map stable aliases to canonical roots and explicit `ro`/`rw` policy. Paths are
`alias:relative/path`; traversal, raw absolute paths, symlink escape, and unregistered aliases fail
closed. New files use `fs.write`; `fs.patch` atomically applies expected-context hunks to an existing
fresh-tagged UTF-8 file and spills oversized diffs; `fs.move` and approval-gated `fs.remove` own
whole-file operations. Patch output preserves uniform LF/CRLF input and reports only bounded
context around changed lines. `proc.run` accepts an argv vector,
allowlisted executable/argument shapes, linked cwd, timeout, and output cap—never a shell string.
On Unix, cancellation and timeout kill the isolated process group, including descendants.

Mutations and unsafe process shapes use `ApprovalPolicy`; default denial and timeout are denial.
Approvals resume the same tm effect continuation and retain the node/session/actor/turn provenance
needed by durable event replay.

## 7.5 Resources and artifacts

The registry owns URI handlers such as `artifact://`, `linked://`, `drive://`, `memory://`,
`skill://`, `agent://`, and `history://`. A handler being registered does not grant read access;
the exact `resources.read:<scheme>` grant is still required. Large results spill to artifacts and
return a bounded reference/preview rather than entering model context wholesale.

## 7.6 No ambient authority

tm exposes no raw filesystem, process, network, environment, package manager, native module,
dynamic evaluation, or secret-value access. Capability handlers receive JSON-shaped arguments and
return JSON-shaped values; sensitive argument previews are redacted in runtime events. Future MCP
imports must remain catalog capabilities behind the P9 egress/secret boundary and must not add
chat-native tools.
