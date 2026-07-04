# 9. Context, artifacts & resource resolution

## 9.1 Artifact spill — big outputs stay out of the window

- Outputs above the per-cell cap are **content-addressed** and stored; the model receives
  `artifact://<id>`, a MIME type, a size, and a short preview.
- The model re-reads on demand: `artifacts.slice(id, start, end)` — paged, never the whole blob.
- Artifacts are referenceable across cells and persist for the session (optionally the workspace).
- This is the mechanism that keeps a 2 MB fetch from ever entering the window: it lands in the
  store, the code works on it in-sandbox, and only `display(summary)` reaches context.

The two-tier store behind these handles — global `blob:sha256:` + session `artifact://` / `agent://`
— is specified in §25.3. This section owns the **read path**: how any handle (and every other
readable thing) is resolved.

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
- **`read(uri, selector?)`** — returns content, **paged via the same selector semantics as
  `artifacts.slice`** (§9.1); never the whole blob in one shot.
- **`list(uri?)`** — optional enumeration (roster / index views).
- **preview + MIME** — only a preview / summary returns until the model slices (**progressive
  disclosure**, §07).


### Client resource gateway

`tm-server` exposes the same registry to product clients through session-scoped HTTP endpoints; clients
do **not** implement scheme-specific handlers. Flutter/Web/Android ask the server to resolve, list, or
preview a URI, and the server applies the same capability gates, selector paging, MIME detection, and
failure semantics as `resources.read`.

Implemented P1/P2 API shape (§27.5, §22.9):

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

Internal schemes (v1). The implemented P1/P2 session gateway currently registers
`artifact://`, `workspace://session`, `linked://`, `project://`, and `memory://`. Other catalog
entries are reserved designs and must fail closed until their backing subsystem registers a handler
and grant.

| scheme | resolves | backing subsystem | registered by |
|---|---|---|---|
| `artifact://<id>` | session-local tool output (monotonic int) | artifact store §25 | `tm-artifacts` |
| `agent://<id>` | sub-agent output artifact | agents §23 / §25 | `tm-agents` |
| `history://<id>` | read-only sub-agent transcript | agents §23 | `tm-agents` |
| `memory://…` | P2 memory gateway: `root`, `user-model`, approved profile facts, and scoped recall chunks (§22.9). Richer `MEMORY.md`, episodic query, project memory, and dream views remain later `tm-memory` work. | memory §22 | `tm-server` memory slice now; `tm-memory` later |
| `skill://<name>` | **Reserved prompt label only in P2**: composed system prompts may label injected `SKILL.md` sections this way, but `resources.read/list/preview("skill://...")` is not registered and must fail closed until P4/P7 defines the read lifecycle. | skills §22 / §26 | prompt composition now; no resource handler until P4/P7 |
| `drive://<path>` | user document by canonical path §24 | drive §24 | `tm-drive` |
| `cron://[<id>]` | scheduler job table: list jobs / a job's def + run history §27.2 | scheduler §27 | `tm-server` |
| `workspace://session/<path>` | current session sandbox workspace read/list/preview | sandbox workspace §07 / §08 | `tm-sandbox` |
| `linked://<alias>/<path>` | explicitly granted local/remote folder under an `FsPolicy` grant | host adaptor §25 | `tm-host` |
| `project://<id>/<view>` | aggregate project surface: status, open loops, decisions, linked folders, artifacts, agents | server project layer §27 / memory §22 / host §25 | `tm-server` |

`skill://` is **not yet promoted** from prompt-composition provenance to a first-class resource
scheme. The P2 persona layer injects active skill markdown directly into the system prompt and labels
those sections with `skill://<name>` for source clarity. That label is not model-readable through the
SDK or client gateway: `resources.read("skill://...")`, `resources.preview("skill://...")`, and
`resources.list("skill://...")` must fail closed as unknown schemes. P4/P7 may promote it later, but
only with a registered handler, scheme-specific grant, provenance, audit/replay, and safe
import/version/reload rules.

### Workspace, linked folders, and projects

`workspace://session/<path>` is the read/list/preview route for the current session's sandbox workspace
jail (§07 / §08). It is session-scoped like `artifact://`: the session id in the resource gateway
selects the actual workspace, and the URI stays relative to that workspace. It is for Miku's temporary
scratch files and generated intermediates, not for user documents, host files, or long-lived memory.

`linked://<alias>/<path>` is the read/list/preview route for an explicitly linked folder. It is backed
by the same `FsPolicy` grant used by `fs.*`, `code.*`, and `proc.run` (§25), but the resource route is
read-only: writes still go through `fs.write` / `code.edit`, and commands still go through
`proc.run(cmd,args)` with argv allowlists and approvals. The alias is stable and model-visible; raw
host paths are not. A remote folder uses the same `linked://` URI — "remote" is a connector property
behind the grant, not a public scheme.

`project://<id>/<view>` is an aggregate product view, not a generic host filesystem. It composes project
memory, the session event log, linked-folder registry, artifacts, agents, and promoted workspace
attachments into stable surfaces such as `project://tempestmiku`,
`project://tempestmiku/open-loops`, `project://tempestmiku/decisions`,
`project://tempestmiku/linked-folders`, `project://tempestmiku/workspace`, and
`project://tempestmiku/resources`. It links to `memory://`, `linked://`, `artifact://`, and
`agent://` resources rather than replacing them.

### Project promotion

Promotion copies selected session-scoped state into a durable project while preserving provenance. It is
not a URI rename and it never writes to a linked host folder unless a separate `fs.write` / `code.edit`
operation is approved.

| source | promoted target |
|---|---|
| `workspace://session/<path>` | `project://<id>/workspace/<path>` |
| `artifact://<id>` | `project://<id>/artifacts/<id>` |
| session summary / open loops / decisions | `memory://projects/<id>/…`, surfaced through `project://<id>/…` |
| `linked://<alias>/` | listed under `project://<id>/linked-folders` |

Promoted workspace files are backed by the project attachment store (content-addressed blobs plus a
project manifest) until `tm-drive` can optionally materialize them into `drive://`. Each promoted entry
records `source_uri`, source session, content hash, timestamp, and actor. Existing targets default to
keep-both; overwrites require explicit approval.

### Parked — external read-through resources (§15)

`issue://` · `pr://` (GitHub) and `mcp://` (MCP server **resources**, §25.1) are **deferred**. They
differ in kind from the internal schemes: a **network-egress read-through proxy** needs the egress
allowlist (§08), a credential via the **secret broker** (§08.3), and a **disk cache** — and the core
declared UI / deployment out of scope. They slot into the **same registry behind the same handler
contract** when enabled; until then they remain an open question (§15).

## 9.4 Failure modes & degradation

- **Unknown scheme** → error listing the registered schemes.
- **Reserved prompt-only label** such as `skill://...` → unknown scheme until the owning milestone
  registers a handler and grants.
- **Capability denied** → **fail closed** (§08); the read never runs.
- **Handler resolves nothing** (id / path missing) → not-found error listing available ids / paths
  (mirrors the artifact-store behavior, §25.5).
- **(Parked external)** offline → return cache; credential missing → fail closed.
