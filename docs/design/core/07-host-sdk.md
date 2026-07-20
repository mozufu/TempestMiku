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

## 7.3 Available namespaces

- `tools.search`, `tools.docs`, `tools.call` — bounded catalog discovery/late-bound invocation.
- `resources.read`, `resources.preview`, `resources.list` — capability-gated access to registered
  URI schemes.
- `artifacts.put` — intrinsic content-addressed session output and spill references.
- `artifacts.get`, `artifacts.slice`, `artifacts.list` — session artifact inspection, available only
  with the explicit `resources.read:artifact` grant.
- `http.get`, `http.request`, `secrets.use` — the default-disabled, exact-destination HTTPS egress
  and opaque-secret boundary. Non-GET requests require approval.
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

### 7.3.1 Production egress

`tm-egress` is the concrete transport behind `http.get`, `http.request`, and `secrets.use`. An enabled
destination fixes HTTPS scheme, host, port, path prefixes, methods, redirect edges, caller header
names, request/response limits, and a positive policy version. Calls require the SDK capability plus
the exact `egress.destination:<id>` grant; opaque secret handles additionally require
`secrets.use:<id>`. Configuration never emits wildcard authority, and a child actor receives these
names only through explicit delegation from a parent that already holds them.

Every hop resolves and validates DNS before connecting, pins the validated address set, disables
automatic redirects, and re-authorizes the next origin/path/method. Private, loopback, non-routable,
link-local, and metadata addresses fail closed; link-local and known metadata endpoints remain hard
denied even when a destination explicitly opts into private addresses. Atomic session and
destination reservations cover request count, request bytes, maximum response bytes, and timeout.
The server persists those counters and outstanding reservations transactionally, keyed by session
and destination id, so an active session cannot reset a cap through process restart, policy-version
rotation, or concurrent API instances. Only bounded UTF-8 response text is returned.

Every non-GET `http.request` first resolves a value-free immutable policy/handle snapshot, then
shows a bounded redacted query/body semantic preview plus canonical query, request, and target
digests and non-secret destination/secret id/version metadata. The snapshot is resolved again after
approval. A changed, revoked, expired, or differently scoped handle fails before DNS. The durable
effect row is inserted before transport and reaches `succeeded`, `failed`, or `uncertain` before the
completion audit. Retrying an identical host-owned turn effect returns its receipt; a persisted
`started`/`uncertain` effect is never resent after origin loss.

`secrets.use` returns a session/actor/version-bound opaque token. The actual environment value is
resolved only while constructing an authorized host request, held in zeroizing memory, scoped to its
configured destinations, and redacted from returned text and bounded host-only metadata. A restart
does not restore tokens. Runtime policy uses immutable per-request snapshots, so revocation/reload
does not wait for a slow peer and all later requests see the new generation before DNS. Acceptance
evidence is recorded in `docs/evidence/2026-07-18-p9-egress-secret-broker.md`.

Literal occurrences of the injected value are removed from bounded response fields. This is not a
general information-flow proof: an authenticated peer can transform, encode, hash, or summarize a
credential before reflecting data, which exact-value redaction cannot identify. Destinations that
receive credentials are therefore owner-trusted endpoints; untrusted fetched content remains data
and must not be granted an authenticated destination merely to rely on response redaction.

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
check. Cancellation and timeout kill the fresh process group and its ordinary descendants. A
deliberately detached descendant that calls `setsid(2)` can escape portable process-group
containment, so every invocation remains manually approval-gated.

Linux deployments may additionally select the fail-closed `linux_bubblewrap` profile. It requires a
root-owned non-writable launcher and explicit root-owned runtime roots, descriptor-pins the linked
root and cwd into bubblewrap, exposes only those mounts, unshares user/mount/PID/IPC/UTS/network
namespaces, drops capabilities, clears ambient environment, and applies bounded address-space,
process-count, and open-file rlimits. A missing or changed profile aborts before approval/spawn and
never falls back to direct host execution. The profile is disabled by default and does not remove
manual approval. Its executable canary is evidence for this namespace/rlimit level only.

The stronger opt-in `linux_hardened_v1` profile retains that boundary and additionally requires a
sealed repo-owned architecture-specific `developer_v1` seccomp program plus an exclusive writable
delegated cgroup-v2 subtree. Each execution receives an unpredictable leaf with CPU, memory, swap,
and pids limits; the child joins before exec, and success/timeout/cancel/drop kill, drain, account,
and remove the leaf. Startup exposes an explicit orphan-recovery operation for exact service-owned
leaf names; `tm-server` invokes it before constructing its API or worker runtime and aborts startup
on failure. Policy version/digest/architecture, delegated-root identity, and all limits are bound
into approval/profile identity. Missing policy, controllers, delegation, or pinned roots fail before
approval with no lower-profile fallback. Disposable Linux/aarch64 and native x86_64 canaries close
the software profiles. The selected homolab production contract additionally proves persistent
systemd delegation, representative sizing/headroom, clean cgroup ownership, native execution, and
restart stability. That accepted boundary is a hostile workload on a trusted owner-controlled host
kernel; hostile-kernel containment and microVM isolation are not claimed.

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
approval; sensitive argument/result previews are redacted in runtime events. MCP imports remain
catalog capabilities behind the same egress/secret boundary and do not add chat-native tools.
