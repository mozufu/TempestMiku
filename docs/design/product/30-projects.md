# 30. Projects — subject entities, and what drive means

> **Current contract.** Server-owned project entities, `project.link/unlink` grant attachments,
> entity-rooted memory-scope authority with archive tombstones, drive as Miku's playground, and
> session assignment replacing promotion are live. §22/§24/§27/§09 match; this doc is the single
> home of the decision.

## 30.0 The decision

Three words had collapsed into one: a "project" was simultaneously a linked-folder alias, a memory
scope, an aggregate view, and a drive attribute — and it could not exist without a host folder, while
every linked folder was forced to become one. Drive was designed as "my files" but pulled toward
per-project workspaces. Promotion tried to glue the gaps and became a batch re-implementation of
three mechanisms that already exist. The owner-decided meanings:

- **Project** — *something with a specific subject.* A first-class, server-owned, durable entity.
  It usually has a folder linked in, but the folder is an **optional attachment** (0..n), not the
  project's existence proof.
- **Drive** — *Miku's playground.* The durable space of content shared with Miku: what Brian uploads,
  and what Miku files away as worth keeping. Not a Google-Drive replacement and not Brian's
  filesystem — "shared with Miku" is a physical boundary (content enters only through an explicit
  put), not an ACL.
- **Linked folder** — a pure **capability grant** attached to a project. One of two doors through
  which Miku can see user content (the other is drive). Linking creates no project and no memory
  scope.
- **Promotion** — demoted to **session assignment**: declaring that a session belongs to a project.
  No batch copy, no special import path.

## 30.1 The two doors (unchanged architecture, honest vocabulary)

Miku's access to user content has exactly two doors, and the object-capability substrate (§24.0)
already enforced this; only the product vocabulary drifted:

| door | enters via | authority |
|---|---|---|
| **Drive** (playground) | explicit `drive.put` — upload, import, or Miku filing | server-owned store; scope-checked |
| **Linked folder** | `project.link(host_path, mode)` | unforgeable `FsPolicy` grant attached to a project |

Content outside both doors is invisible to Miku. Privacy therefore needs no in-drive ACL: what should
stay private simply never enters.

## 30.2 The project entity

Server-owned durable record:

- **Identity:** stable slug id, display title, `active | archived` status, created/updated timestamps,
  and an immutable default memory policy (`project` by default, optionally `global`) used only when a
  caller selects this project without specifying a session memory policy.
- **Lifecycle:** created explicitly (owner action, or an approval-gated Miku proposal — including
  "new project" chosen inside the link flow); **archive** hides the project from pickers and new
  sessions and tombstones its memory scope (§30.4).
- **Attachments:** 0..n linked-folder grants (`project.link` / `project.unlink`), sessions assigned
  to it, drive entries referencing it, and project items (summaries / open loops / decisions / next
  actions) grown by the per-turn observation pipeline (§27).
- **Surfaces:** `GET /projects` lists **entities** (never aliases) without host or connector details;
  `project://<id>/<view>` composes memory, items, sessions, artifacts, agents, declarative environment
  cognition, and attached links; the memory scope `project:<slug>` is opened at entity creation.
  The environment view is capability-gated, project/session scoped, and rechecks active project
  status on every native read; global, wrong-project, and archived-project reads fail closed.

A project may exist with no folder (planning-only, docs-only). A folder may move path without the
project noticing: detach the old grant, attach the new one — memory and items are untouched because
they were never the folder's contents.

## 30.3 Linking, attenuation, revocation

- `project.link(host_path, ro | rw)` — approval-gated host call; registers the `FsPolicy` grant and
  attaches it to an existing project entity. Replaces `drive.link`, which never touched the document
  store.
- `project.unlink(alias_or_uri)` — approval-gated detach; revokes **only** the filesystem grant.
- `fs.*` / `code.*` / `proc.*` authority requires **both** the session's matching `project_id` and a
  live attached grant; sessions without a project fail closed (§24.4). Memory policy is irrelevant
  to this check.
- Durable-link fail-closed restart semantics (existence, canonical identity, non-symlink root, mode,
  alias revalidation) are unchanged (§24.4).
- The `drive_linked` / `drive_unlinked` session-event types retire with the move; link events become
  project events.

## 30.4 Memory scope lifecycle (re-pointed revocation)

The durable memory authority contract — durable tombstones, serialized scope revocation with every
typed-memory / embedding-provenance / generation / pointer / job write (migrations 0017–0018), and
exact replay consulting the same durable authority — is preserved and rooted in the project entity:

- the revocation trigger is project **archive** (entity lifecycle), not folder unlink;
- unlink no longer tombstones, cancels embedding jobs, or denies scoped reads;
- sessions that still select the archived project or its memory scope fail closed against the entity
  status and durable tombstone.

This is a deliberate authority-root move: memory-scope authority was rooted in a live filesystem
grant; it is now rooted in the server-owned project record. Filesystem authority stays grant-rooted.
The two revocation axes are independent, and each fails closed on its own trigger.

## 30.5 Drive consequences (§24 amendments)

- `entry.project` becomes a **validated project-entity reference** when set. Filing with an unknown
  project proposes creating the entity (approval-gated) or files unprojected; it never auto-creates.
- Canonical paths never embed the project: the `projects/{project}/{docKind}/{filename}` default
  convention is retired; the per-project view stays on the `/by-project/<project>` virtual directory.
  "Attributes are the index, not the folder" (§24.3) now applies to projects too.
- Existing `drive://projects/<slug>/...` entries are historical canonical paths and are **not**
  rewritten (same hard-cut discipline as the tm-lang cutover); only new filings follow the new
  conventions.
- Drive is bidirectional: Brian's uploads and Miku's filed outputs enter through the same put path,
  the same transduction, and the same approval policy.

## 30.6 Session assignment replaces promotion

- Active session → `POST /sessions/:id/scope` independently updates optional `projectId` and
  `memoryPolicy`; `project` policy requires a project. When policy is omitted, the selected project's
  immutable default applies, or `global` when no project is selected.
- Closed session → `POST /projects/:id/sessions/:session_id` attaches it for project observation; the
  server re-runs the per-turn extraction (§27) over its event log. Project items grow through one
  mechanism regardless of when assignment happened.
- User-initiated assignment is the approval; Miku-initiated assignment emits a `write_proposal` and
  waits for approval.
- Retired and removed (clean cutover, no shims): `POST /sessions/:id/promote`,
  `importResourcesToDrive`, the `drive://projects/<id>/attachments/` convention, and new
  promoted-pointer item kinds (existing records are historical and untouched). Keeping a session
  output is ordinary approval-gated `drive.put` with `sourceUri` provenance; clients may
  batch-select, but that is a client concern, not a server concept.

## 30.7 Memory pools — symmetric single-pool cross-project recall

**Status: storage/API implemented; recall fan-out pending.** The `memory_pools` entity, the
`pool_id` column on `projects`, both store backends, and the join/leave/archive HTTP surface are
live. The fan-out step below — widening `memory.search` candidate generation across active
pool-member scopes — is the remaining rollout step (§30.8 Migration shape).

**Motivation.** A project can depend on others — e.g. an app project (`SlimeOS`) built from two
library projects (`zutai`, `dango`). The owner works across all three and wants Miku's recall to
surface relevant memory from the sibling projects without merging their scopes or granting ambient
access.

- **Memory pool** — a durable, server-owned entity: stable id, display name, `active | archived`
  status. Membership is **project ↔ pool**, and — a deliberate simplicity choice scoped to the
  concrete need above, not a general N-pool model — **each project belongs to at most one active
  pool at a time**. `pool.join(project_id)` / `pool.leave(project_id)` are approval-gated host calls,
  the same shape as `project.link` / `project.unlink` (§30.3).
- **What pool membership does not change:** a project's own `memory_scope` (`project:<id>`), its
  writes, its exact reads (`memory://records/...`, `memory://recalls/...`), and its archive tombstone
  lifecycle (§30.4) are exactly as before. Pool membership never widens write authority or
  exact-record reads — only the fuzzy hybrid recall path below.
- **What it does change:** when the active scope is `project:<id>` and that project is an active
  member of a pool, `memory.search`'s candidate generation (§22.3) additionally queries every other
  **active** member project's scope in the same fan-out step. Each candidate keeps its source scope
  as provenance so Miku can attribute and cite correctly. RRF fusion (§22.3) applies a lower default
  weight to cross-pool candidates than same-scope candidates, so a project's own memory dominates its
  ranking instead of being diluted by pool siblings.
- **Revocation is a pure read-time filter:** a project leaving its pool, a pool being archived, or the
  member project itself being archived (§30.4) all drop that project out of every remaining member's
  fan-out set on the *next* query. The fan-out set is recomputed from current active
  membership/status per query, so this needs no new tombstone type — it reuses the existing
  scope/status gate that already runs before every scoped read.

If a second concrete case ever needs a project to sit in more than one pool, or needs asymmetric
(one-way) visibility, revisit the single-pool constraint above rather than generalizing preemptively.

## 30.8 Migration shape

1. `projects` table + entity CRUD + lifecycle tombstones; backfill one entity per active link alias.
2. `project.link` / `project.unlink` host calls; `drive.link` / `drive.unlink` removed with every
   caller (tm docs, server routes, tests, Flutter) migrated in the same change.
3. Memory-scope authority re-pointed at the entity record; archive serialization reuses the
   0017–0018 machinery; the unlink path stops tombstoning.
4. Session-assignment endpoints; the promote endpoint and `importResourcesToDrive` removed;
   observation catch-up for closed sessions.
5. Drive: validated project references, new filing conventions, unknown-project proposal flow.
6. Flutter: the entity picker and dedicated Project/Drive/History pages replace drawer expansion;
   active sessions select scope, closed sessions expose assignment, and Drive presents the
   scope-relative playground without treating a linked folder as the project itself.
7. Memory pools (§30.7): `memory_pools` table + entity CRUD; `pool.join` / `pool.leave` host calls —
   **done**. Recall candidate-generation fan-out across active pool-member scopes with provenance
   tagging and lower cross-pool RRF weight — **pending**; no changes to write authority, exact
   reads, or archive tombstones.

Every migration step preserves the established drive/memory acceptance boundaries or amends them
explicitly in the same change.
