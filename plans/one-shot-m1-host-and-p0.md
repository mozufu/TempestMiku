# One-shot implementation plans: M1 + host, then P0

Created from the 2026-06-27 planning discussion. Use this as source material for `/next-milestone` or a fresh long-session prompt. `ROADMAP.md` remains canonical for milestone order.

## Current decisions

- Work in a separate worktree/branch for planning changes; do not mutate an active implementation worktree.
- First implementation target: **M1 + host foundation**.
- Next product target after that substrate: **P0 character server slice**.
- Do not start P1/P2+ until M1+host and P0 acceptance checks pass.

---

## Milestone 1: M1 + host foundation

### Status

- [x] Done

### Outcome

Complete the M1 substrate plus host foundation without starting the full P0 product slice.

### Scope

- Add `tm-artifacts` for repo-local artifact/blob storage.
- Add `tm-host` for host functions, capability grants, resource resolution, and approval policy.
- Add a real `deno_core` sandbox backend behind existing `Sandbox` / `Session` traits.
- Support TypeScript and JavaScript cells.
- Preserve persistent top-level state across cells.
- Implement session reset.
- Enforce per-cell timeout/cancel behavior.
- Implement `display` / `print` capture.
- Implement output caps with artifact spill and preview shaping.
- Implement `artifact://<id>` reads through the resource registry.
- Implement default-deny `http.get` with allowlist support.
- Wire a generic host-call bridge for future SDK namespaces.
- Keep persona assets configurable by path; do not vendor current deployment assets.
- Add Postgres-facing foundation only where needed for the host/memory substrate; normal tests must not require a running DB.

### Explicit non-goals

- Do not build full P0 character chat.
- Do not build full `tm-memory` recall/dreaming.
- Do not build `tm-agents`, `tm-drive`, Android, or self-evolution tiers.
- Do not add raw shell execution or `sh -c` escape hatches.
- Do not bypass the one model-visible `execute(code)` architecture.

### Implementation decisions

- Storage default for repo work: repo-local `.tempestmiku/`.
- Artifact ids: session-local monotonic `artifact://<id>` handles.
- Blobs: content-addressed global store under repo-local storage.
- TS support: real TS cells, not JS-only.
- Postgres tests: environment-gated integration tests plus pure unit fallback; normal `cargo test` must pass without Postgres.
- Security stance: no ambient filesystem/network/process access; every host operation is capability-checked and fail-closed.
- Server framework if a stub becomes necessary: `axum`.
- Frontend default if a WebUI smoke surface becomes necessary: Vite + React.

### Acceptance checks

All must pass before marking this milestone done:

- [x] `cargo test` passes without external services.
- [x] Tests prove TypeScript cell execution.
- [x] Tests prove persistent state across cells.
- [x] Tests prove reset clears state.
- [x] Tests prove timeout/cancel returns a structured error and does not kill the host.
- [x] Tests prove `display` / `print` capture reaches shaped output.
- [x] Tests prove large output spills to `artifact://` with bounded preview.
- [x] Tests prove `artifact://` resolves through the registry.
- [x] Tests prove unknown resource schemes fail closed.
- [x] Tests prove unknown capabilities fail closed.
- [x] Tests prove blocked network by default.
- [x] Tests prove allowlisted `http.get` works or is covered by a deterministic local/test handler.
- [x] Tests prove approval timeout/default-deny behavior.
- [x] No frontend/WebUI files were added, so Playwright smoke is not applicable.

### Fresh-session prompt

Use this plan as the active implementation brief. First read `AGENTS.md`, `ROADMAP.md`, and the relevant design docs: §06, §07, §08, §09, §25, and §27. Implement only **M1 + host foundation**. Preserve the streaming-first loop, one model-visible `execute(code)` architecture, `Sandbox` / `Session` trait boundary, no raw shell, fail-closed security model, repo-local artifact storage, configurable persona asset paths, and normal `cargo test` without external services. Do not start the full P0 product slice. Verify every acceptance check before marking the milestone done.

---

## Milestone 2: P0 character server slice

### Status

- [x] Done

### Outcome

Ship a minimal, dogfoodable Miku chat slice over `tm-server` + WebUI, backed by Postgres session/event/memory storage, without building P1 engineer capabilities or full memory dreaming.

### Scope

- Add `tm-server` with an `axum` custom session API and SSE stream.
- Add a chat-only WebUI with Vite + React.
- Use Postgres as the single backing store for sessions, events/replay, messages, profile facts, and recall chunks.
- Load persona assets from a configurable path.
- If persona assets are missing, boot degraded with a warning instead of failing boot.
- P0 mode behavior: default Personal Assistant badge only; do not implement full router yet.
- Inject minimal memory context from profile facts + recall chunks.
- Support forwarded auth / reverse-proxy identity where configured; local dev may use token/no-auth mode.
- Add Playwright smoke for the chat stream and replay path.

### Explicit non-goals

- Do not implement full 5-mode router in P0.
- Do not implement full `tm-memory` dreaming, graph, RRF, or skill generation.
- Do not implement P1 linked-folder repo editing.
- Do not implement `tm-agents`, `tm-drive`, Android, or self-evolution tiers.
- Do not make SQLite or file logs a second replay source; use Postgres.

### API shape

- `POST /sessions` creates a session.
- `GET /sessions/:id/events` streams SSE events.
- `POST /sessions/:id/messages` sends a user message into the session.
- `GET /health` reports server status.

### SSE events

Use §27 event names and keep them replayable:

- `text` — assistant token delta.
- `tool_call` — `{name}`.
- `cell_start` — `{code}`.
- `cell_result` — shaped result.
- `mode` — default Personal Assistant badge/status.
- `final` — final assistant text.
- `error` — server/session error.

Each SSE frame must include a monotonic event id. Reconnect uses `Last-Event-ID` and replays missed events from Postgres.

### Postgres minimum schema

Expandable toward §22, but P0 only needs:

- `sessions`
  - `id`
  - `created_at`
  - `updated_at`
  - `status`
  - `mode`
  - `persona_status`
- `session_events`
  - `session_id`
  - `seq`
  - `event_type`
  - `payload_json`
  - `created_at`
- `messages`
  - `session_id`
  - `seq`
  - `role`
  - `content`
  - `created_at`
- `profile_facts`
  - `id`
  - `subject`
  - `predicate`
  - `object`
  - `confidence`
  - `provenance`
  - `valid_from`
  - `valid_to`
- `recall_chunks`
  - `id`
  - `scope`
  - `text`
  - `source`
  - `created_at`
  - optional `embedding` later

### WebUI behavior

- Chat-only screen.
- Renders streamed token deltas, not only final text.
- Shows default mode badge.
- Reconnect path uses replay/resume.
- No memory browser, drive browser, agent roster, or self-evolution review surface in P0.

### Acceptance checks

All must pass before marking P0 done:

- [x] `cargo test` passes.
- [x] Server tests prove session creation, message append, event append, and event replay by `Last-Event-ID`.
- [x] Server tests prove persona missing path boots degraded with warning/status.
- [x] Server tests prove minimal memory context injection from profile facts + recall chunks.
- [x] Server tests prove forwarded auth behavior or local dev token/no-auth behavior, depending on config.
- [x] Playwright smoke opens the chat page, sends a message, observes streaming token deltas, observes final text, reconnects, and observes replay/resume.

### Fresh-session prompt

Use this plan as the active implementation brief after M1 + host foundation is complete. First read `AGENTS.md`, `ROADMAP.md`, and design docs §21, §22, §27, and §29. Implement only **P0 character server slice**: `tm-server` with `axum`, Postgres-backed sessions/events/messages/profile/recall, configurable persona asset path with degraded boot on missing assets, default Personal Assistant badge, chat-only Vite + React WebUI, and Playwright smoke covering stream + final + reconnect replay. Do not implement full router, full memory dreaming, P1 engineering capabilities, agents, drive, Android, or self-evolution. Verify every acceptance check before marking the milestone done.

---

## Milestone 3: P1 serious engineer permissions

### Status

- [ ] Pending after P0 character server slice

### Outcome

Add the first serious-engineer real-repo capability surface: config-declared linked folders, yolo-by-default project commands/writes like Pi (`pi.exe`), and patch-edit only. This is the first milestone where TempestMiku can edit and test a real repo through curated SDK calls, without adding raw shell or broad code-intelligence tooling yet.

### Scope

- Add config-declared linked folders. No WebUI folder picker in this milestone.
- Add `FsPolicy` grants from config: linked path, alias/project name, mode (`ro` / `rw`), and allowed commands.
- Add `fs.*` real-repo access only through linked-folder grants.
- Add `proc.run(cmd, args)` using argv-vector execution only; never `sh -c` or shell strings.
- Add default yolo behavior like Pi (`pi.exe`) for configured linked folders: safe build/test/check commands and normal writes run without per-action approval once the config grants them.
- Keep approvals for explicitly destructive, external, or out-of-grant actions.
- Add first `code.edit` as patch/surgical edit only.
- Defer LSP and AST editing/search to later milestones.

### Explicit non-goals

- Do not add raw terminal access.
- Do not add shell command strings.
- Do not add a WebUI linked-folder picker yet.
- Do not implement LSP rename/references/diagnostics in P1 first pass.
- Do not implement AST grep/edit in P1 first pass.
- Do not implement agents, handoff fan-out, or drive auto-organizer here.

### Config decisions

- Linked folders are declared in config, not selected interactively:

```toml
[[linked_folders]]
name = "tempestmiku"
path = "/path/to/repo"
mode = "rw"
commands = ["cargo", "pnpm"]
safe_args = [
  ["cargo", "test"],
  ["cargo", "fmt"],
  ["cargo", "clippy"],
  ["pnpm", "test"],
  ["pnpm", "playwright", "test"],
]
```

- The exact config format may be TOML/YAML/JSON to match the implementation, but it must preserve the fields above.
- `proc.run` matches command plus argv prefix against the configured allowlist.
- A configured `rw` grant permits normal `fs.write` / patch edit within the linked root.
- Path traversal and symlink escape fail closed.

### Acceptance checks

All must pass before marking P1 serious engineer permissions done:

- [ ] `cargo test` passes.
- [ ] Tests prove linked folders load from config.
- [ ] Tests prove `fs.read` / `fs.write` cannot escape the linked root.
- [ ] Tests prove a configured safe command runs without approval.
- [ ] Tests prove a non-allowlisted command is rejected.
- [ ] Tests prove command execution uses argv-vector semantics, not shell parsing.
- [ ] Tests prove destructive/external/out-of-grant operations still require approval or fail closed.
- [ ] Tests prove patch edit applies only inside a linked `rw` folder.
- [ ] Tests prove patch edit rejects stale/invalid anchors or out-of-root paths.

### Fresh-session prompt

Use this plan as the active implementation brief after P0 is complete. First read `AGENTS.md`, `ROADMAP.md`, and design docs §08, §21, §24, and §25. Implement only **P1 serious engineer permissions first pass**: config-declared linked folders, `FsPolicy`, yolo-by-default safe `proc.run(cmd,args)` for configured project commands, linked-folder `fs.*`, and patch-only `code.edit`. Preserve no raw shell, argv-vector execution, linked-root confinement, fail-closed security, and one model-visible `execute(code)` architecture. Defer LSP and AST tooling to later milestones. Verify every acceptance check before marking this milestone done.

---

## Future grill queue

These are intentionally not resolved here:

1. Deployment mode: foreground CLI, launchd service, Docker, or lumo service.
2. Full mode router implementation after default P0 badge.
3. LSP support milestone: references, rename, diagnostics, code actions.
4. AST support milestone: ast-grep and ast-edit SDK shape.
5. Agents/handoff fan-out milestone.
