# 9. Context, artifacts & resource resolution

## 9.1 Artifact spill — big outputs stay out of the window

- Outputs above the per-cell cap are **content-addressed** and stored; the model receives
  `artifact://<id>`, a MIME type, a size, and a short preview.
- The model re-reads on demand: `resources.read(artifact://<id>, selector)` — paged, never the whole blob.
- Artifacts are referenceable across cells and persist for the session (optionally the workspace).
- This is the mechanism that keeps a 2 MB fetch from ever entering the window: it lands in the
  store, the code works on it in-sandbox, and only `display(summary)` reaches context.

The two-tier store behind these handles — global `blob:sha256:` + session `artifact://`, with actor
resources at `agent://` / `history://` using the spill path as needed — is specified in §25.3. This
section owns the **read path**: how any handle (and every other readable thing) is resolved.

Storage references are validated before path construction: session ids are 1–128 safe ASCII
characters and reject empty/absolute/`.`/`..`; artifact ids are canonical decimal `u64`; blob ids are
exactly 64 lowercase hexadecimal characters after `blob:sha256:`. Blob reads reject symlinks/reparse
points and paths outside the root, then rehash the bytes and fail integrity on mismatch. Blob creation
is atomic/no-clobber, and artifact metadata id/URI must match its filename.

Quotas are hard boundaries: 4 MiB per text artifact, 64 MiB per blob, and 256 MiB aggregate logical
storage per session. A global deduplicated blob is charged once to each session that references it;
reusing the same blob in that session does not charge it twice. Text reads stream rather than loading
the file/all lines; default selection is at
most 64 KiB or 200 lines, with a hard request maximum of 256 KiB or 1,000 lines. Unknown ids release
the store mutex before building diagnostics, so not-found reporting cannot deadlock writers/readers.

## 9.2 Resource resolution — one registry, scheme-keyed handlers

Everything readable is addressed through a uniform `read(uri)` (SDK `resources.read(uri)`),
**dispatched by URI scheme to a registered handler**. This generalizes the per-subsystem
`resources` / `resolve` modules (§22 / §23 / §24 / §25) into **one registry**: each subsystem
**registers** its handler at startup; the registry knows nothing about any subsystem (late-binding,
principle #9 — handlers are **config, not a hardcoded router**). A new readable capability ⇒ register
a handler, never patch a central switch.

### Two families — do not conflate

| Family | Examples | Resolved | Model-addressable? |
|---|---|---|---|
| **Persistence reference** | `blob:sha256:<hash>`, the spill side of `artifact://` | at **load** (rehydration) | no — storage internal |
| **Resource route** | `agent://` `memory://` `cron://` … | **on demand** via `read(uri)` | yes |

`blob:` is the hash of content rehydrated when a transcript loads (§25.3) — **not** a router URL. The
schemes catalogued below are the *resource* family: live, capability-checked, paged reads.

### Handler contract

Every handler implements the same shape (`ResourceHandler`, §10.2):

- **`scheme`** — the URI scheme it owns; one registry entry per scheme.
- **capability gate** — checked at the boundary like every host fn (§07 / §08); a scheme the run
  lacks authority for **fails closed**.
- **`read(uri, selector?)`** — returns content, **paged via selector semantics**
  (§9.1); never the whole blob in one shot.
- **`list(uri?)`** — optional enumeration (roster / index views).
- **preview + MIME** — only a preview / summary returns until the model slices (**progressive
  disclosure**, §07).


### Client resource gateway

`tm-server` exposes the same registry to product clients through session-scoped HTTP endpoints; clients
do **not** implement scheme-specific handlers. Flutter/Web/Android ask the server to resolve, list, or
preview a URI, and the server applies the same capability gates, selector paging, MIME detection, and
failure semantics as `resources.read`.

Implemented client resource-gateway API shape (§27.5, §22.9):

| endpoint | purpose |
|---|---|
| `GET /sessions/:id/resources/resolve?uri=...&selector=...` | page a resource body |
| `GET /sessions/:id/resources/list?uri=...` | enumerate a scheme/root/index view |
| `GET /sessions/:id/resources/preview?uri=...` | return title, kind, MIME, size, and short preview |

Responses use a typed envelope rather than raw, scheme-specific payloads:

```json
{
  "uri": "artifact://3",
  "kind": "text",
  "mime": "text/plain",
  "title": "cargo test output",
  "size_bytes": 184223,
  "selector": "1-200",
  "has_more": true,
  "preview": "running 42 tests...",
  "content": "..."
}
```

Binary/image resources return preview/download URLs in the envelope instead of pushing bytes through the
event stream. SSE events carry only URI + preview metadata; the UI resolves on demand. This keeps large
outputs out of both the model context and the mobile client render path.

## 9.3 Resource catalog

Internal and selected read-through schemes (v1). The session gateway currently registers `artifact://`,
`workspace://session`, `linked://`, `project://`, `memory://`, `agent://`, `history://`, `cron://`,
`drive://` when the local-first drive store is configured, and `skill://` when the managed catalog is
configured. The gateway additionally registers `mcp://` only when a trusted selected-import config passes
egress validation and startup discovery. Other catalog entries are reserved designs and must fail closed
until their backing subsystem registers a handler and grant.

| scheme | resolves | backing subsystem | registered by |
|---|---|---|---|
| `artifact://<id>` | session-local tool output (monotonic int) | artifact store §25 | `tm-artifacts` |
| `agent://<id>` | sub-agent output/record resource | agents §23 / §25 | `tm-agents` |
| `history://<id>` | read-only sub-agent transcript | agents §23 | `tm-agents` |
| `memory://…` | bounded root/user-model views, approved profile/scoped records, dream/summaries/proposals, typed episodic/semantic records, and exact turn recall provenance (§22.9); query-shaped ambient `MEMORY.md` and episodic-query paths remain absent | memory §22 | `tm-memory` contracts + `tm-server` gateway |
| `skill://[<name>[/versions[/<digest>]]]` | the managed-skill catalog, active body, version metadata, and immutable digest-addressed body. Bundled/hand-authored skills are prompt assets and are not exposed through this managed catalog. | skills §22 / §26 | `tm-modes` handler + `tm-server` gateway |
| `drive://<path>` | a user document by canonical path, with virtual directory views such as `drive://by-project/<project>` and `drive://by-type/<kind>` §24 | drive §24 | `tm-drive` |
| `cron://[<id>]` | the scheduler job table: list jobs / a job's definition + run history §27.2 | scheduler §27 | `tm-server` session resource gateway |
| `workspace://session/<path>` | current session workspace read/list/preview | workspace §07 / §08 | `tm-server` |
| `linked://<alias>/<path>` | explicitly granted local/remote folder under an `FsPolicy` grant | host adaptor §25 | `tm-host` |
| `project://<id>/<view>` | aggregate project surface (§30 entity): status, open loops, decisions, attached linked folders, artifacts, agents | server project layer §30 / memory §22 / host §25 | `tm-server` |
| `mcp://<server>/resources/<source-uri-digest>` | selected remote MCP resource returned as bounded untrusted data with catalog/target/payload provenance; the remote source URI is not exposed | MCP / egress §07 | `tm-mcp` + `tm-server` when configured |

Managed skills are promoted to a first-class resource scheme without opening a `skills.*`
write namespace. `skill://` lists active managed entries; `skill://<name>` reads the active body;
`skill://<name>/versions` returns version state; and
`skill://<name>/versions/<sha256-hex>` reads an immutable version. The native tm handler requires
`resources.read:skill`; the authenticated session gateway applies the same parser and catalog source.
An unconfigured catalog remains unregistered, selectors are rejected, and bundled/hand-authored
skills cannot be shadowed by managed entries.

### Workspace, linked folders, and projects

`workspace://session/<path>` is the read/list/preview route for the current session's sandbox workspace
jail (§07 / §08). It is session-scoped like `artifact://`: the session id in the resource gateway
selects the actual workspace, and the URI stays relative to that workspace. It is for Miku's temporary
scratch files and generated intermediates, not for user documents, host files, or long-lived memory.

`linked://<alias>/<path>` is the read/list/preview route for an explicitly linked folder. It is backed
by the same `FsPolicy` grant used by `fs.*`, `code.*`, and `proc.run` (§25), but the resource route is
read-only: writes still go through `fs.write` / `fs.patch`, and commands still go through
`proc.run(cmd,args)` with argv allowlists and approvals. The alias is stable and model-visible; raw
host paths are not. A remote folder uses the same `linked://` URI — "remote" is a connector property
behind the grant, not a public scheme.

`project://<id>/<view>` is an aggregate product view, not a generic host filesystem. It composes project
memory, the session event log, assigned sessions, attached linked-folder grants, artifacts, agents,
and workspace attachments into stable surfaces such as `project://tempestmiku`,
`project://tempestmiku/open-loops`, `project://tempestmiku/decisions`,
`project://tempestmiku/memory`, `project://tempestmiku/linked-folders`, `project://tempestmiku/workspace`, and
`project://tempestmiku/resources`. It links to `memory://`, `linked://`, `artifact://`, and
`agent://` resources rather than replacing them. `project://<id>/linked-folders/<alias>/...`
delegates to the same linked-folder resource handler as `linked://<alias>/...`, preserving selector
paging and fail-closed `FsPolicy` checks while presenting the entry inside the project view.
`project://<id>/memory` exposes the explicit `memory://scopes/project:<id>/chunks` pointer for
project-scoped recall while `memory://root` remains the active session-scope view.

The authenticated `GET /projects` catalog and the session resource listing root `project://` expose
active project **entities** (§30), each with zero or more attached linked folders. Each catalog entry
supplies the stable project URI, matching `project:<id>` memory scope, immutable default memory
policy, and `project://<id>/linked-folders` collection URI. The catalog never serializes canonical
host paths, worker endpoints, command allowlists, or other connector details; unlink detaches a
folder but never removes the project. Entering a project or reading a linked-folder child requires
the session's exact server-owned `project_id`, independently of its memory policy.

The Flutter project picker presents each catalog project as one flat root. Selecting
`<id>` lists `project://<id>/linked-folders/<id>/` directly when a same-name folder is attached, so
the internal `linked-folders` collection and same-name alias do not appear as redundant navigation
levels; a project with no attached folder opens its memory/items views instead. Generic resource
browsers may still enumerate the collection URI explicitly.

### Session assignment (promotion retired, §30)

Promotion as a batch copy operation is retired (§30): it duplicated the session-scope endpoint, the
per-turn project-observation pipeline, and plain `drive.put` filing in one special endpoint that
could only target link-backed projects. The surviving server concept is **session assignment** —
declaring that a session belongs to a project:

- An active session independently updates its optional `projectId` and `memoryPolicy` through
  `POST /sessions/:id/scope`; turns with a project id grow project items automatically via the
  per-turn observation pipeline (§27), regardless of memory policy.
- A closed session can be attached to a project retroactively; the server re-runs the same
  observation extraction over the session's event log. Nothing is copied or renamed.
- Keeping a session output is ordinary approval-gated drive filing: `drive.put` with `sourceUri`
  provenance and an optional validated project reference. There is no `importResourcesToDrive` path
  and no `drive://projects/<id>/attachments/` convention.

### Selected external read-through resources (§15)

Operator-selected `mcp://` resources use the same handler contract. The handler is
registered only after bounded startup discovery succeeds; reads require the exact selected resource,
egress destination, and optional opaque-secret grants. Remote content is wrapped as
`mcp_untrusted_resource` / `mcp_untrusted_data` with protocol revision, catalog generation/digest,
local server alias, object identity, target/payload digests, and bounded byte count. The authenticated
session gateway persists its MCP and egress audit sequence in the normal replayable event log, while
list/preview receive no network or secret grants.

Selected MCP read-through deliberately has no offline content cache: peer outage or revocation fails
closed. Process startup is the configuration/reload boundary, preventing cached interpreter sessions
from retaining an older binding generation. `issue://` and `pr://` remain parked until a concrete
consumer supplies its selected handler and cache semantics.

## 9.4 Failure modes & degradation

- **Unknown scheme** → error listing the registered schemes.
- **Managed skill catalog unconfigured** → `skill://...` is absent from the registered scheme list.
- **Capability denied** → **fail closed** (§08); the read never runs.
- **Handler resolves nothing** (id / path missing) → not-found error listing available ids / paths
  (mirrors the artifact-store behavior, §25.5).
- **Configured MCP peer offline / invalid / revoked, or credential missing** → fail closed; no
  stale content, ambient credential, or redirect fallback is returned.
- **MCP result** → always bounded and marked untrusted with local provenance; remote descriptions
  and initialize instructions never become local docs or instructions.
