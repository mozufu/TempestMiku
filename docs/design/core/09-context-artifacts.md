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
| **Resource route** | `agent://` `memory://` `skill://` `cron://` … | **on demand** via `read(uri)` | yes |

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

## 9.3 Resource catalog

Internal schemes (v1):

| scheme | resolves | backing subsystem | registered by |
|---|---|---|---|
| `artifact://<id>` | session-local tool output (monotonic int) | artifact store §25 | `tm-artifacts` |
| `agent://<id>` | sub-agent output artifact | agents §23 / §25 | `tm-agents` |
| `history://<id>` | read-only sub-agent transcript | agents §23 | `tm-agents` |
| `memory://…` | memory store (root / `MEMORY.md` / user-model / episodic …) §22.9 | memory §22 | `tm-memory` |
| `skill://<name>` | skill source / `SKILL.md` (procedural playbook) §26 / §07 | skills §22 / §26 | `tm-memory` |
| `drive://<path>` | user document by canonical path §24 | drive §24 | `tm-drive` |
| `cron://[<id>]` | scheduler job table: list jobs / a job's def + run history §27.2 | scheduler §27 | `tm-server` |

`skill://` is **promoted** from the nested `memory://…/skills/<name>` path to a first-class scheme:
same store (§22 *Skills* row), direct address.

### Parked — external read-through resources (§15)

`issue://` · `pr://` (GitHub) and `mcp://` (MCP server **resources**, §25.1) are **deferred**. They
differ in kind from the internal schemes: a **network-egress read-through proxy** needs the egress
allowlist (§08), a credential via the **secret broker** (§08.3), and a **disk cache** — and the core
declared UI / deployment out of scope. They slot into the **same registry behind the same handler
contract** when enabled; until then they remain an open question (§15).

## 9.4 Failure modes & degradation

- **Unknown scheme** → error listing the registered schemes.
- **Capability denied** → **fail closed** (§08); the read never runs.
- **Handler resolves nothing** (id / path missing) → not-found error listing available ids / paths
  (mirrors the artifact-store behavior, §25.5).
- **(Parked external)** offline → return cache; credential missing → fail closed.
