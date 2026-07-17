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
delegated grants through `opts.capabilities` on `agents.run/spawn/parallel/pipeline`. Omitted options
delegate nothing, including the former read-only `http.get` + `resources.read:artifact` pair. Every
requested name must already be held by the parent and be delegable; `backend.*` and `modes.*` remain
server control planes and cannot be delegated. The list is bounded to 32 names, 128 ASCII bytes per
name, and 2 KiB total.

## 7.3 Shipped namespaces

- `tools.search`, `tools.docs`, `tools.call` — bounded catalog discovery/late-bound invocation.
- `resources.read`, `resources.preview`, `resources.list` — capability-gated access to registered
  URI schemes.
- `artifacts.put` — intrinsic content-addressed session output and spill references.
- `artifacts.get`, `artifacts.slice`, `artifacts.list` — session artifact inspection, available only
  with the explicit `resources.read:artifact` grant.
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

Artifact storage bounds both bytes and namespace shape: per-session artifact/blob counts,
aggregate artifact metadata, blob-reference metadata, and directory entries all fail closed.
`artifacts.list` accepts `{offset?, limit?}`, defaults to 100 entries, and caps each page at 256;
resource-registry listing is likewise capped at 256. Async host paths move artifact open/read/write
and listing scans onto blocking workers so a bounded disk scan cannot stall the runtime executor.

## 7.4 Real-repo boundary

Linked folders map stable aliases to canonical roots and explicit `ro`/`rw` policy. Paths are
`alias:relative/path`; traversal, raw absolute paths, symlink escape, and unregistered aliases fail
closed. The registry pins the linked root's device/inode identity, so replacing the configured path
with another real directory also fails until the owner explicitly relinks it. On Unix, reads,
list/find/search walks, and mutation parents use descriptor-relative no-follow traversal; other
platforms fail closed rather than falling back to check-then-open paths.
New files use `fs.write`; `fs.patch` atomically applies expected-context hunks to an existing
fresh-tagged UTF-8 file and spills oversized diffs; `fs.move` and approval-gated `fs.remove` own
whole-file operations. Patch output preserves uniform LF/CRLF input and reports only bounded
context around changed lines. Repo-controlled `.gitignore` loading is also bounded before glob
compilation: 1 MiB file input, 64 patterns, 4 KiB per pattern, and 64 KiB aggregate pattern bytes.
Recursive linked-folder walks reject trees deeper than 128 directory levels and stop after 100,000
visited entries. List/find/search responses are charged by their exact serialized JSON size,
including string escapes, commas, and array framing, and never exceed 4 MiB.
Mutations are serialized per linked-folder registry and recheck policy revision, tag, and
device/inode identity at commit. POSIX has no portable inode-conditional
rename/unlink primitive, so an unrelated host process can still race the final identity check and
rename/unlink; descriptor anchoring prevents that race from escaping the linked root, but it is not
a cross-process compare-and-swap guarantee. A registry-wide policy gate orders bounded linked reads,
final mutation syscalls, and process validation/spawn against policy replacement or removal. Thus a
policy change either lands first and makes the old-revision operation fail, or waits until that
already-validated read/commit/spawn finishes; it cannot slip between the final check and syscall.

`proc.run` accepts an argv vector, allowlisted executable names, bounded argument shape, linked cwd,
timeout, and output cap—never a shell string. The host drops empty/relative `PATH` entries, resolves
an absolute executable plus identity before approval, rechecks it afterward, and enters the approved
cwd by descriptor in the child. Optional stdin is capped at 1 MiB; its approval action binds
presence, raw byte count, SHA-256 of the raw bytes, and a redacted preview capped at 256 bytes with
an explicit truncation marker. On Unix, a final allocation-free `pre_exec` hook re-stats the
canonical executable and rejects a device/inode mismatch immediately before path exec. Platforms
without an fd-based exec primitive still have an unavoidable stat-to-exec race after that final
check, so process execution remains a manually approved, non-isolated boundary. Cancellation and
timeout kill the fresh process group and its ordinary descendants. A deliberately detached
descendant that calls `setsid(2)` can escape portable process-group containment; every invocation
therefore remains approval-gated until stronger platform isolation exists.

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
return JSON-shaped values. `ToolDocs.sensitive` marks trace/persistence privacy and does not imply
approval; sensitive argument/result previews are redacted in runtime events. Future MCP
imports must remain catalog capabilities behind the P9 egress/secret boundary and must not add
chat-native tools.
