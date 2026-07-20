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

- **Identity:** stable slug id, display title, `active | archived` status, created/updated
  timestamps.
- **Lifecycle:** created explicitly (owner action, or an approval-gated Miku proposal — including
  "new project" chosen inside the link flow); **archive** hides the project from pickers and new
  sessions while preserving memory for exact reads; **delete** is the only scope-killing transition
  and writes the durable tombstone (§30.4).
- **Attachments:** 0..n linked-folder grants (`project.link` / `project.unlink`), sessions assigned
  to it, drive entries referencing it, and project items (summaries / open loops / decisions / next
  actions) grown by the per-turn observation pipeline (§27).
- **Surfaces:** `GET /projects` lists **entities** (never aliases) without host or connector details;
  `project://<id>/<view>` composes memory, items, sessions, artifacts, agents, and attached links;
  the memory scope `project:<slug>` is opened at entity creation.

A project may exist with no folder (planning-only, docs-only). A folder may move path without the
project noticing: detach the old grant, attach the new one — memory and items are untouched because
they were never the folder's contents.

## 30.3 Linking, attenuation, revocation

- `project.link(host_path, ro | rw)` — approval-gated host call; registers the `FsPolicy` grant and
  attaches it to an existing project entity. Replaces `drive.link`, which never touched the document
  store.
- `project.unlink(alias_or_uri)` — approval-gated detach; revokes **only** the filesystem grant.
- `fs.*` / `code.*` / `proc.*` authority still requires **both** the matching project scope **and** a
  live attached grant; global sessions fail closed (§24.4).
- Durable-link fail-closed restart semantics (existence, canonical identity, non-symlink root, mode,
  alias revalidation) are unchanged (§24.4).
- The `drive_linked` / `drive_unlinked` session-event types retire with the move; link events become
  project events.

## 30.4 Memory scope lifecycle (re-pointed revocation)

The durable memory authority contract — durable tombstones, serialized scope revocation with every
typed-memory / embedding-provenance / generation / pointer / job write (migrations 0017–0018), and
exact replay consulting the same durable authority — is preserved and rooted in the project entity:

- the revocation trigger is project **archive/delete** (entity lifecycle), not folder unlink;
- unlink no longer tombstones, cancels embedding jobs, or denies scoped reads;
- sessions already inside a revoked scope still fail closed, now against the entity tombstone.

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

- Active session → the existing `POST /sessions/:id/scope`.
- Closed session → `POST /projects/:id/sessions/:session_id` attaches it; the server re-runs the
  per-turn observation extraction (§27) over the session's event log. Project items grow through one
  mechanism regardless of when assignment happened.
- User-initiated assignment is the approval; Miku-initiated assignment emits a `write_proposal` and
  waits for approval.
- Retired and removed (clean cutover, no shims): `POST /sessions/:id/promote`,
  `importResourcesToDrive`, the `drive://projects/<id>/attachments/` convention, and new
  promoted-pointer item kinds (existing records are historical and untouched). Keeping a session
  output is ordinary approval-gated `drive.put` with `sourceUri` provenance; clients may
  batch-select, but that is a client concern, not a server concept.

## 30.7 Migration shape

1. `projects` table + entity CRUD + lifecycle tombstones; backfill one entity per active link alias.
2. `project.link` / `project.unlink` host calls; `drive.link` / `drive.unlink` removed with every
   caller (tm docs, server routes, tests, Flutter) migrated in the same change.
3. Memory-scope authority re-pointed at the entity record; archive/delete serialization reuses the
   0017–0018 machinery; the unlink path stops tombstoning.
4. Session-assignment endpoints; the promote endpoint and `importResourcesToDrive` removed;
   observation catch-up for closed sessions.
5. Drive: validated project references, new filing conventions, unknown-project proposal flow.
6. Flutter: the entity picker and dedicated Project/Drive/History pages replace drawer expansion;
   active sessions select scope, closed sessions expose assignment, and Drive presents the
   scope-relative playground without treating a linked folder as the project itself.

Every migration step preserves the established drive/memory acceptance boundaries or amends them
explicitly in the same change.
