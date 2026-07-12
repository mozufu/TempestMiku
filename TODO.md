# TODO

Last aligned: **2026-07-12**.

Active milestone: **P6 push foundation landed; production provider canary next**.

`ROADMAP.md` remains the canonical milestone order. The P4/P5 mechanism slices and their deterministic
acceptance coverage have landed, as has the production-hardening implementation: supervised roles,
durable approvals/effects, fenced leases, ordered checksummed migrations, authoritative session
scope, durable turns/SSE replay, and Postgres drive metadata/link state. The complete automated and
physical-device verification matrix now passes, so P4/P5 are production-complete for the documented
single-owner deployment. P6 now also has encrypted provider-neutral push registration/outbox and
authenticated Android notification actions, while real remote delivery remains open. Keep this checklist aligned with core docs
§05/§06/§07/§09/§11 and product docs §21, §22, §23, §24, §25, §27, and §29.

## P5 North Star

Ship the first useful local-first document space and research workspace:

- The user can drop or put a file into TempestMiku's drive without granting ambient real-FS access.
- The system extracts useful field-value attributes, proposes a canonical path, and files only after
  the applicable approval/tier policy allows it.
- `drive://` resources are readable through the same capability-gated resource registry and client
  gateway as `artifact://`, `memory://`, `agent://`, `history://`, `cron://`, and `linked://`.
- `drive.*` is exposed as a host capability namespace through the existing one-tool `execute(code)`
  architecture, not as a second orchestration path.
- Linking a real project folder mints an `FsPolicy` grant and opens the matching per-project memory
  scope as one approval-bound act.
- Deep research can combine P3+ agents, P4 memory/summaries/scheduler, artifacts, and filed drive
  documents while preserving bounded context and provenance.

P5 is not an Android, MCP, self-evolution, or generic cloud-drive milestone. It layers `tm-drive` on
the existing host registry, resource gateway, approval broker, memory store, event log, and clients.

## Non-Negotiable Invariants

- The model still gets one chat-native tool: `execute(code)`.
- Streaming remains the source of truth. Every user-visible event uses the versioned
  `event: session_event` envelope with durable numeric sequence, `turnId`, payload, and timestamp.
- Capabilities are config and grants, not prompts. Unknown capability names and unknown or ungranted
  resource schemes fail closed; handler registration never grants authority and every turn replaces
  rather than unions its exact grant set.
- `drive://` is a user-document resource route, not a host path and not a blob hash. Raw host paths
  must never become model-visible.
- Default drive storage is sandbox/local-first. Real folders are reachable only through explicit
  `drive.link` / configured linked-folder policy and approval where interactive.
- `linked://<alias>/...` remains the read-only resource view of an `FsPolicy`; writes still go
  through `fs.write`, `code.edit`, or explicit drive operations with approval.
- `proc.run(cmd,args)` remains argv-vector only. No shell-string shortcut, no ambient cwd, no `sh -c`.
- Large file contents, extraction logs, research corpora, and sub-agent outputs spill to artifacts,
  blobs, or resource rows. Do not push them wholesale into model context or SSE payloads.
- Approval deny/timeout writes nothing, revokes no extra state, and remains replayable.
- Normal `cargo test` stays external-service-free. Live LLM, Postgres, browser/Flutter, network, and
  cloud-sync coverage stays opt-in or gated.

## Current Baseline

- [x] P0/P1/P2 are complete per `ROADMAP.md`.
- [x] P3 and P3-plus are complete:
      `agents.run/spawn/parallel/msg/send/broadcast/wait/inbox/list/cancel/pipeline`, live inboxes,
      active supervision, child approvals, replayable actor/resource provenance, and client smoke
      coverage are in place.
- [x] P4 mechanisms exist: `tm-memory`, dream queue/worker, summaries, skill proposals, Postgres FTS
      coverage, cron jobs/runs, `cron://` resources, and replayable proactivity are in place.
- [x] `tm-host` already owns linked-folder `FsPolicy`, `fs.*`, `code.*`, `proc.*`, and `linked://`
      resource behavior.
- [x] `tm-server` already exposes session-scoped resource resolve/list/preview endpoints and
      registers current resource schemes.
- [x] `drive://` now registers through `tm-drive` when a drive store is configured and still fails
      closed when unregistered or ungranted.
- [x] `tm-drive` crate exists.
- [x] `drive.*` exists in the runtime SDK.
- [x] Drive metadata, transducers, virtual dirs, organizer proposal records, and first project
      memory-scope recall coupling exist.

## Production Hardening Gate

- [x] Repository CI covers formatting, strict clippy, workspace tests, gated Postgres tests,
      Flutter analysis/tests, Flutter Web, Android debug builds, Playwright smoke, and Cargo audit.
- [x] Replace startup schema bootstrap with ordered, checksummed migrations and upgrade/backfill tests.
- [x] Make session end, lifecycle events, and dream enqueue one atomic durable transition.
- [x] Persist approval requests and resumable effects; cancel non-resumable waits cleanly on restart.
- [x] Fence dream and cron claims by owner/epoch, heartbeat active work, and reject stale completion.
- [x] Supervise turn/dream/scheduler/effect workers through `api|worker|all` runtime roles with
      graceful shutdown; require Postgres for `worker` and `all`.
- [x] Enforce cron deny-only approval mode, turn budget, exact capabilities, and timeout in code.
- [x] Use Postgres-backed drive metadata/organizer/link state when Postgres is configured, and hydrate
      validated active links on startup.
- [x] Add deterministic and gated restart/two-store tests for leases, approvals, drive state, and
      legacy-schema upgrades.
- [x] Complete the local Rust, clean-schema Postgres, split-process crash/restart, Flutter,
      authenticated Playwright, signed Android debug/release build, release-certificate, RustSec,
      and secret-leak gates.
- [x] Run the final authenticated Android device/emulator canary over HTTPS: scan and confirm a
      one-time pairing code, chat through a durable turn, reconnect SSE without gaps/duplicates, and
      prove revocation immediately removes access.

Final verification commands/evidence:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=postgres://... cargo test -p tm-server
cargo audit
nix develop --command bash -lc 'cd clients/miku_flutter && flutter analyze && flutter test'
cd clients/miku_web && npm test
git diff --check
```

The local two-process Postgres recovery case, authenticated Playwright pairing/replay smoke, arm64
debug/release APK builds, and non-debug release certificate check pass. The physical Android 15
release canary over Tailscale Serve HTTPS proved QR confirmation, durable chat, cold-start recovery,
exact-once replay of a turn completed while the app was stopped, SSE termination/send disablement
after revocation, and a clean post-canary secret scan.

## P5 Mechanism Acceptance Gate

- [x] A dropped or `drive.put(..., { auto: true })` file is stored locally, transduced into
      field-value attributes, deduped by content hash, assigned a proposed canonical path, and filed
      only after approval/tier policy allows it.
- [x] `drive.get`, `drive.ls`, `drive.search`, `drive.tag`, `drive.move`, `drive.link`, and
      `drive.organize` work through capability-checked host calls with denial/default-deny tests.
- [x] `drive://<path>` can be resolved/listed/previewed through the normal session resource gateway
      with paging, MIME, preview metadata, grants, and fail-closed behavior.
- [x] Virtual directories such as `/by-project/<project>`, `/by-type/<kind>`, and `/recent` map to
      attribute queries without moving canonical files.
- [x] Linking a project folder mints or reuses an `FsPolicy` and opens the matching per-project memory
      scope; revocation/narrowing invalidates both together.
- [x] Filed documents become recallable through the P4 memory/retrieval surface with provenance back
      to `drive://`, source session/event ids, and content hashes.
- [x] The research workspace can fan out over drive documents with P3+ agents and return only
      bounded digests/resource refs to the parent context.
- [x] Offline local-first operation works without cloud or network dependency.
- [x] Normal `cargo test` passes; gated Postgres/client/live tests document their env vars.

## P5.0 API Contract And Data Model

- [x] Freeze the first P5 data vocabulary before writing host ops:
      drive entry id, canonical path, blob/content hash, MIME, size, title, doc kind, project,
      entities, dates, amounts, tags, embedding placeholder, source URI, provenance, created/updated
      timestamps, and status.
- [x] Define canonical path rules:
      normalized separators, no `..`, no raw host paths, stable casing policy, collision strategy
      (`keep-both` by default), and explicit overwrite approval.
- [x] Define virtual directory grammar:
      `/recent`, `/by-project/<project>`, `/by-type/<doc_kind>`, `/by-tag/<tag>`,
      `/by-date/<yyyy>/<mm>`, and a fallback query object for future extensions.
- [x] Define the first `DrivePutOptions`:
      `auto`, `suggestedPath`, `project`, `docKind`, `tags`, `sourceUri`, `mime`, `title`,
      `approvalMode`, and `dedupe`.
- [x] Define the first `DriveSearchOptions`:
      `project`, `docKind`, `tags`, `limit`, `includeArchived`, `since`, `until`, and
      `returnSnippets`.
- [x] Define organizer proposal records:
      proposed move/tag/dedupe action, evidence, confidence, policy decision, approval id,
      status, source run id, and replay metadata.
- [x] Decide exact `session_event.data.type` values and payloads before client work:
      `drive_put`, `drive_transduced`, `drive_path_proposed`, `drive_write_proposed`,
      `drive_filed`, `drive_moved`, `drive_tagged`, `drive_linked`, `drive_organizer_started`,
      `drive_organizer_completed`, and `drive_organizer_failed`.
- [x] Add JSON wire tests for every new event/resource payload before exposing it to clients.

Acceptance:

- [x] API/data shapes are documented in `docs/design/product/24-drive-storage.md`,
      `docs/design/core/07-host-sdk.md`, `docs/design/core/09-context-artifacts.md`, and
      `docs/sdk/tm-runtime.d.ts`.
- [x] Unknown future fields are either ignored safely or rejected with a stable error.

## P5.1 Crate And Workspace Setup

- [x] Add `crates/tm-drive` to the workspace and workspace dependencies.
- [x] Keep `tm-drive` concrete and boring; do not introduce broad framework abstractions before two
      users exist.
- [x] Add module skeletons matching §24.6:
      `store`, `transduce`, `organize`, `vdir`, `policy`, `resources`, and `types`.
- [x] Define crate-local errors with `thiserror`; keep `anyhow` at binary/server edges only.
- [x] Export only the stable P5 surface from `tm-drive::lib`.
- [x] Add deterministic unit-test fixtures under the crate for text, markdown, JSON, image/blob refs,
      duplicate content, and malformed paths.
- [x] Add `tm-drive` to `docs/design/core/10-rust-implementation.md` crate plan once behavior lands.

Acceptance:

- [x] `cargo test -p tm-drive` runs without external services.
- [x] The crate compiles as a standalone workspace package (`cargo test -p tm-drive`) while exposing
      explicit host/server adapter APIs.

## P5.2 Local-First Store And Blob Integration

- [x] Implement the in-memory `DriveStore` first:
      put/get/list by canonical path, update attrs, move, archive/delete marker if needed, search by
      indexed attributes, organizer proposal lifecycle, and idempotent writes by content hash.
- [x] Store document bytes by content hash using the existing artifact/blob primitives where possible;
      avoid a second CAS implementation unless the existing blob layer cannot fit.
- [x] Store drive entries as metadata pointing at `blob:sha256:` or session/project artifact refs;
      never duplicate binary content unnecessarily.
- [x] Add integrity checks on read: content hash mismatch fails closed.
- [x] Add size, MIME, preview, and selector/paging helpers consistent with `ResourceContent`.
- [x] Add Postgres schema behind gated tests only after in-memory behavior is stable:
      `drive_entries`, `drive_attributes`, `drive_tags`, `drive_proposals`, `drive_links`, and
      indexes for path, hash, project, doc kind, tags, recency, and FTS text.
- [x] Preserve local-first semantics: no cloud dependency and no live network in normal tests.
- [x] Add import path for promoted project attachments so `project://.../workspace` resources can
      optionally materialize into `drive://`.

Acceptance:

- [x] The same logical drive tests pass against in-memory store and gated Postgres store.
- [x] Duplicate content creates one blob and separate metadata only when paths differ.
- [x] Offline read/list/search works entirely from local store state.

## P5.3 Transducers And Attribute Extraction

- [x] Implement deterministic fallback transducers first:
      plain text, markdown, JSON, filename/path hints, MIME, size, content hash, created timestamp,
      simple date/entity/tag heuristics, and project/doc-kind hints from options.
- [x] Add type-specific extractors behind traits so model-assisted extraction can be added without
      changing store/resource contracts.
- [x] Add model-role hooks for richer extraction only behind config:
      document classification, entities, dates, amounts, summary, and embedding generation.
- [x] Run redaction before model-assisted extraction or durable summaries:
      secrets, private keys, auth headers, obvious credentials, and sensitive PII patterns.
- [x] Ensure transducer failure degrades to MIME + filename + recency and never loses the file.
- [x] Attach provenance to every extracted attribute:
      extractor version, evidence snippet/resource selector, source URI, session id, and confidence.
- [x] Add extraction budget limits for file size, token preview, number of attributes, and evidence
      snippets.
- [x] Add tests for malformed files, binary files, redaction, fallback behavior, confidence scoring,
      bounded snippets, and deterministic output.

Acceptance:

- [x] `drive.put` can classify representative notes, receipts/invoices, papers, and project docs
      without live LLM access.
- [x] Model extraction can be disabled and the acceptance path still works.

## P5.4 Canonical Paths, Virtual Directories, And Search

- [x] Implement placement proposer:
      attributes + user/project conventions -> proposed canonical path.
- [x] Add configurable conventions with safe defaults:
      `projects/<project>/<doc-kind>/<title>`, `finance/<yyyy>/<doc-kind>/...`, and
      `inbox/<yyyy-mm-dd>/...` fallback.
- [x] Ensure user corrections through `drive.move` and `drive.tag` are recorded as learning signals
      for future placement without silently rewriting old files.
- [x] Implement `vdir` query mapping:
      virtual path -> conjunctive attribute filter.
- [x] Implement `drive.ls` over both canonical path prefixes and virtual dirs.
- [x] Implement `drive.search` as hybrid lexical/attribute search first, reusing P4 FTS/recency/
      importance patterns where practical.
- [x] Add snippet generation that returns bounded evidence and `drive://` selectors.
- [x] Add path collision handling:
      keep-both, explicit move, explicit overwrite approval, and stale-source rejection.
- [x] Add tests for virtual dirs, canonical path listing, search ranking, snippets, collision handling,
      move correction, and query/path ambiguity.

Acceptance:

- [x] `/by-project/X` and `/by-type/invoice` return the expected entries without moving files.
- [x] Search returns bounded results with resource refs and does not load full documents into context.

## P5.5 `drive://` Resource Handler

- [x] Implement `tm-drive::resources` handler for `drive://<path>`.
- [x] Support `read(uri, selector?)`, `preview(uri)`, and `list(uri?)` with the same envelope shape as
      existing resource gateway responses.
- [x] Gate reads through `resources.read:drive`, previews through `resources.preview:drive`, and lists
      through `resources.list:drive` or the equivalent existing grant shape.
- [x] Keep reserved `drive://` paths fail-closed until the handler is registered.
- [x] Return stable not-found errors with nearby available paths only when that does not leak
      ungranted information.
- [x] Add binary/image behavior:
      preview metadata and download/resource handles, not raw bytes in SSE or model context.
- [x] Add selector paging for large text docs.
- [x] Register the handler in `tm-server` session resource gateway only when drive is configured.
- [x] Add resource gateway tests for resolve/list/preview, grants, denial, unknown scheme,
      not-found, selector paging, MIME, and binary preview.

Acceptance:

- [x] Client and sandbox resource reads share the same authorization and pagination semantics.
- [x] Existing `artifact://`, `memory://`, `agent://`, `history://`, `cron://`, `workspace://`, and
      `linked://` resource tests do not regress.

## P5.6 Host Capability Namespace: `drive.*`

- [x] Add host catalog docs for `drive.put`, `drive.get`, `drive.ls`, `drive.move`, `drive.search`,
      `drive.tag`, `drive.link`, `drive.unlink`, and `drive.organize`.
- [x] Expose the namespace through the existing sandbox SDK generator/runtime path; no chat-native
      tool additions.
- [x] Add grants:
      `drive.put`, `drive.get`, `drive.ls`, `drive.move`, `drive.search`, `drive.tag`, `drive.link`,
      `drive.unlink`, `drive.organize`, and resource read/list/preview grants for `drive://`.
- [x] Decide whether `drive.get` returns `ResourceContent` directly or a `drive://` ref plus preview;
      prefer bounded `ResourceContent` with selectors for parity with resources.
- [x] Ensure every mutating call uses approval policy when required:
      file create, move, overwrite, link, organizer apply, tag edits that affect memory recall, and
      project-scope coupling.
- [x] Ensure deny/timeout writes nothing and emits proposal status where user-visible.
- [x] Add SDK typings in `docs/sdk/tm-runtime.d.ts` for all request/response shapes.
- [x] Add sandbox denial tests for missing capability, unknown method, invalid path, oversized input,
      raw host path, and approval timeout.

Acceptance:

- [x] A Deno cell can `await drive.put(...)`, inspect the returned `drive://` ref, and later
      `await resources.read(ref)` under grants.
- [x] Runs without drive grants cannot infer document existence or host paths.

## P5.7 Link Policy And Project Memory Scope Coupling

- [x] Reuse or extend `tm-host::FsPolicy` instead of creating a parallel real-FS permission model.
- [x] Register approved `drive.link` calls into the shared in-process `LinkedFolders` registry so
      existing `linked://` resources and `fs.*` boundaries see the new alias.
- [x] Implement `drive.link(host_path, mode)` as an approval-gated operation that mints or registers
      an `FsPolicy` with alias, canonical root, mode, and empty commands/safe args for dynamic links.
- [x] Implement `drive.unlink(alias_or_uri)` as an approval-gated revocation operation returning
      alias, canonical root, `linked://` URI, memory scope id, and revocation timestamp.
- [x] Couple link creation to a per-project memory scope:
      one approved link grants both filesystem access and scoped memory recall/write surface.
- [x] Add filesystem attenuation/revocation:
      same-root relink can narrow `rw -> ro`, widening requires approval, and revocation invalidates
      `linked://` resource plus `fs.*` access.
- [x] Add memory-scope attenuation/revocation:
      narrowing or removing a link must invalidate matching memory scope access together with file
      access.
- [x] Ensure linked folders remain exposed as `linked://<alias>/...`, not `drive://`.
- [x] Add project view/resource integration for linked folders:
      linked folders list and read under `project://<id>/linked-folders/...` using the existing
      linked resource handler.
- [x] Surface linked project memory scope through existing `memory://`/`project://` views.
- [x] Add tests for link approval, deny, timeout/default-deny, attenuation, revocation, and project
      linked-folder list/read integration.
- [x] Add tests for duplicate alias, path vanished, symlink escape, and linked-folder fail-closed
      behavior in the link lifecycle.
- [x] Add memory-scope isolation tests for the link lifecycle.

Acceptance:

- [x] A linked project can be used by Serious Engineer through existing `fs.*`/`code.*`/`proc.*`
      boundaries and by drive/search through scoped metadata without ambient host access.
- [x] Removing or narrowing the link fails closed for both file and memory surfaces.

## P5.8 Organizer Worker And Approval Flow

- [x] Implement organizer proposal generation:
      better path, tags, doc kind, project assignment, dedupe, archive suggestion, and evidence.
- [x] Add worker lease/heartbeat/complete/fail behavior matching P4 dream/scheduler patterns to avoid
      duplicate organizer runs.
- [x] Add `drive.organize()` manual trigger plus optional scheduled run hook; scheduler integration
      must use the existing event/session path.
- [x] Gate apply-vs-propose by config/tier:
      conservative default proposes and asks; only low-risk configured classes may auto-apply.
- [x] Reuse approval broker and write-proposal surfaces; do not invent a drive-only approval channel.
- [x] Emit replayable organizer events and resource refs for proposed changes.
- [x] Add stale proposal handling when the source file moved or changed hash.
- [x] Add tests for proposal generation, approval apply, deny, timeout, stale source, duplicate worker
      race, retry/backoff, and low-risk auto-apply config.

Acceptance:

- [x] Dropped files can be auto-filed after approval with exact provenance and replayable events.
- [x] Bad placement can be corrected with `drive.move`, and the correction informs future proposals.

## P5.9 Memory And Recall Integration

- [x] Add drive-derived records to the P4 memory/retrieval path without making `drive` depend on
      server internals.
- [x] Store document summaries/attributes as scoped recall chunks with provenance to `drive://`,
      content hash, extractor version, and source events.
- [x] Keep project-specific docs in project scope; do not leak repo/project lore into global user
      memory.
- [x] Add recall budget rules so session-start memory context can include drive-derived summaries
      without raw document dumps.
- [x] Add contradiction/update behavior:
      moving/tagging a document updates derived recall metadata but preserves historical provenance.
- [x] Add `memory://` previews for drive-derived chunks if they are persisted in memory tables.
- [x] Add tests for scope isolation, recall ranking, move/tag propagation, redaction, approval denial,
      and bounded context injection.

Acceptance:

- [x] A later session in the same project can recall a filed document's summary/open loops without
      loading the full file.
- [x] A different project/global session cannot see project-scoped drive memory without grants.

## P5.10 Research Workspace

- [x] Define the first research workflow:
      select corpus from drive/search, spawn bounded P3+ workers, read paged resources, produce
      digest artifacts, and synthesize a parent answer with citations/resource refs.
- [x] Keep corpus selection explicit and bounded for local corpus shape:
      max docs, max bytes per doc, max snippets, max digest bytes, and max workers.
- [x] Add enforceable per-worker timeouts and total run budget once `agents.parallel` accepts
      per-child budgets.
- [x] Use `agents.pipeline` or `agents.parallel` for fan-out; only child digests and resource refs
      return to parent context.
- [x] Add citation/provenance format:
      `drive://` URI + selector/snippet + content hash + extraction/run id.
- [x] Add research workspace resources under existing project/session surfaces rather than a new
      scheme unless a second concrete user requires one.
- [x] Add approval deferral for external/network research, publishing, sending, or destructive file
      actions.
- [x] Add tests using scripted drive docs and scripted workers for corpus selection, bounded fan-out,
      and citation integrity.
- [x] Add research tests for child failure isolation, cancellation, and replay.

Acceptance:

- [x] A local-only research task can summarize multiple filed documents with citations and without
      unbounded context growth.
- [x] Child-agent failures or cancellations are visible in SSE and do not corrupt drive state.

## P5.11 Server API And Client Surface

- [x] Wire drive store/config into `tm-server` app state.
- [x] Register drive resource handler in the session resource gateway when grants/config allow it.
- [x] Add endpoints only where the generic resource gateway is insufficient:
      prefer `resources/resolve`, `resources/list`, `resources/preview`, and existing approvals.
- [x] Add a compact drive browser feed for clients:
      recent docs, virtual dirs, proposals, pending approvals, and search results.
- [x] Add session/project event payloads for drive actions with mobile-friendly previews.
- [x] Add `apps/tm-e2e` smoke flow:
      put/drop doc -> proposal -> approve -> `drive://` preview -> search -> research digest.
- [x] Add Flutter/Web smoke only after server shapes stabilize:
      browse recent docs, preview `drive://`, resolve pending organizer proposal, reconnect/replay.
- [x] Keep clients thin; they never gain direct write authority over host files, memory, or drive
      metadata outside server-approved APIs.

Acceptance:

- [x] A browser/phone can see a pending drive filing proposal, approve/deny it, open the resulting
      `drive://` resource, and reconnect via `Last-Event-ID`.

## P5.12 Security, Failure Modes, And Degradation

- [x] Unknown `drive.*` methods fail closed with `NotImplementedError` or stable invalid-call errors.
- [x] Unknown/unregistered `drive://` resources fail closed until the handler is enabled.
- [x] Raw absolute host paths in `drive.put`, `drive.get`, `drive.move`, or `drive.search` are rejected
      unless they come through an approved link operation.
- [x] Symlink traversal, `..`, path normalization tricks, duplicate aliases, and stale tags are covered
      by tests.
- [x] Link revocation invalidates shared linked-folder grants and returns a safe error while sandbox
      copies and blobs remain intact.
- [x] Path-vanished linked-folder reads return safe errors without leaking host paths.
- [x] Memory-scope revocation returns safe errors and cannot leak scoped recall.
- [x] Transducer/model failures leave the file stored with fallback metadata and a replayable warning.
- [x] Organizer/store failure leaves proposals retryable or terminal according to config; no partial
      metadata writes.
- [x] Dedup integrity checks use `sha256`; hash mismatch fails closed.
- [x] Sensitive content redaction happens before model extraction, memory writes, or research snippets.
- [x] Network egress remains disabled/default-deny unless P5 explicitly opts into the hardening slice.

Acceptance:

- [x] Security tests cover denial, timeout/default-deny, fail-closed unknown scheme/capability,
      sandbox-default behavior, and linked-folder escape attempts.

## P5.13 Optional HTTP Hardening If Research Needs Live Egress

- [x] Decide early whether P5 research requires live web egress. If not, defer this whole section to
      P7 and keep `http.get` as the deterministic allowlist helper.
- [x] Defer request count, byte caps, timeout caps, redirect policy, response MIME/size filters,
      audit logging, and production allowlists to the future live-egress hardening slice.
- [x] Keep credentials behind future `secrets.use`; do not materialize secret values in JS heap,
      artifacts, model context, or drive metadata.
- [x] Defer network tests to the future live-egress hardening slice; live internet tests remain
      opt-in.
- [x] Ensure research citations distinguish future fetched/external resources from local `drive://`
      docs via `sourceKind`.

Acceptance:

- [x] Live egress remains unavailable in P5; future live-egress work must prove approval, allowlists,
      audit, byte caps, and resource spill before enabling it.

## P5.14 Documentation And Verification

- [x] Update `ROADMAP.md` when the P5 mechanism acceptance checks pass.
- [x] Mark P4/P5 production-complete after the complete automated matrix and physical Android HTTPS
      canary pass.
- [x] Update §24 when concrete store, transducer, organizer, resource, and link behavior lands.
- [x] Update §07 when `drive.*` host catalog/SDK/grant behavior changes.
- [x] Update §09 when `drive://` is registered and no longer only reserved.
- [x] Update §22 when drive-derived memory scopes or recall chunks are implemented.
- [x] Update §23 for `agents.parallel` per-child budgets used by research fan-out.
- [x] Update §27 when client/server drive browser or research workspace surfaces land.
- [x] Update §29 parity notes if drive/research changes user-visible behavior inherited from
      `hermes-agent`.
- [x] Add narrow tests before broad tests:
      store/path, transducers, vdir/search, resource handler, host ops, approvals, link/memory scope,
      organizer, research fan-out.
- [x] Run focused tests as slices land:
      `cargo test -p tm-drive`, `cargo test -p tm-host`, `cargo test -p tm-sandbox`, and
      `cargo test -p tm-server`.
- [x] Run baseline `cargo test` before marking P5 complete.
- [x] Run gated Postgres tests for drive schema/resource/replay work:
      `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=postgres://... cargo test -p tm-server`.
- [x] Run tm-e2e/Flutter smoke only after public API/event shapes change and need client proof.
- [x] Keep live OpenAI/network tests opt-in and skipped by default.

Acceptance:

- [x] P5 can be proven with deterministic scripted tests and no external services.
- [x] Docs describe exactly which drive/research features are implemented and which remain deferred.

## Deferred Namespace Placement

These are roadmap-owned deferred tasks, not loose TODOs:

| Namespace / surface | Target milestone | Placement note |
|---|---|---|
| `drive.*` | **P5 production-complete** | `tm-drive`, virtual dirs, transducers, project memory scopes, resource handling, durable Postgres metadata/organizer/link state, CAS application, startup link hydration, and restart/client verification pass. |
| `drive://` | **P5 production-complete** | The handler, grants, paging, previews, fail-closed tests, client gateway, and Postgres persistence pass; the no-database in-memory metadata path remains the documented local-development exception. |
| `memory.*` richer APIs | **After P5 need is concrete** | P5 should feed existing memory/resource surfaces first. Add new global memory methods only with grants, SDK typings, denial tests, and bounded context behavior. |
| `skills.*` / full `skill://` reads | **P7** | P4 can produce skill proposals. P7 owns import/version/reload, live catalog mutation, MCP import gates, and audit/replay semantics. |
| `http.*` hardening | **P5 or P7** | Do it in P5 only if research needs live egress. Otherwise defer to P7 hardening. |
| `secrets.use` | **P7** | Requires opaque egress-scoped handles and audit guarantees that never expose secret values to JS/model/artifacts/drive. |
| `tm-mcp` | **P7 or explicit later slice** | External resources/tools can feed the same resource/approval model later; do not block local-first drive. |
| Android OS integrations | **P6** | Secure QR/release gates plus encrypted provider-neutral registration, leased approval outbox, fake-provider coverage, and authenticated notification actions have landed. A production FCM/UnifiedPush adapter, remote killed-process canary, and broader OS integration remain. |

## Parallelization Seams

- `tm-drive` store/path/vdir work can proceed independently of host SDK wiring once data shapes are
  frozen.
- Resource handler and server gateway work can proceed with an in-memory drive store before Postgres
  schema lands.
- Deterministic transducers can land before model-assisted extraction and embeddings.
- Organizer proposal generation can land before scheduler integration; manual `drive.organize()` is
  enough for the first acceptance path.
- Research workflow tests can use scripted drive docs and scripted actors before client UI exists.
- Flutter/Web smoke should wait until server event/resource payloads stabilize.

## Do-Not-Start-Yet List

- Remaining Android push/OS integration inside P5; it belongs to P6.
- Cloud sync or CRDT replication in P5 v1; local-first/offline is the acceptance path.
- Generic networked filesystem behavior; drive is the user document space, not ambient host access.
- `skills.*` live import/reload, MCP import gates, and tiered self-evolution writes before P7.
- Secret broker work and production egress hardening unless P5 research explicitly needs the small
  hardening slice.
- A new orchestration loop for research; use the existing agent loop, P3+ actors, resources,
  artifacts, approvals, and event log.
- Any relaxation of manual approval, unknown-scheme fail-closed behavior, argv-vector `proc.run`, or
  linked-folder grant boundaries.

## Crate Plan

Current crates/apps: `tm-core`, `tm-llm`, `tm-sandbox`, `tm-artifacts`, `tm-host`, `tm-modes`,
`tm-agents`, `tm-memory`, `tm-drive`, `tm-server`, `apps/tm-cli`, `apps/tm-e2e`, and clients under
`clients/`.

P5 ownership:

- `tm-drive`: drive entry types, metadata store traits/implementations, path rules, transducers,
  virtual dirs, placement proposer, organizer proposals/worker contracts, resource handler helpers,
  and link/memory-scope policy helpers where they do not depend on server internals.
- `tm-host`: host catalog and dispatch for `drive.*` only where capability registration belongs;
  preserve existing linked-folder `FsPolicy` semantics.
- `tm-sandbox`: SDK namespace exposure, Deno op routing, docs search entries, typings alignment, and
  denial tests.
- `tm-server`: app-state wiring, durable store integration, resource registration, approval broker,
  session events/SSE, project/memory integration, organizer runner, research workflow entry points,
  and client-facing resource/browser APIs.
- `tm-memory`: drive-derived scoped recall records and retrieval hooks if existing P4 surfaces need
  extension.
- `tm-artifacts`: blob/CAS reuse for document bytes and large derived outputs.
- `clients/miku_flutter` and `apps/tm-e2e`: smoke coverage after server contract stabilizes.

Planned product/support crates after P5 remain: `tm-mcp` and `tm-trace`. Avoid extracting more crates
until a second concrete user exists.

## Deferred Product Questions

The audit has no open implementation decisions. Later roadmap work may decide:

- production push-provider selection/integration, physical killed-process delivery canary, and broader Android OS integration as the next P6 slice;
- cloud-drive synchronization/conflict UX after the local Postgres authority is proven;
- pgvector/graph/LLM-backed memory retrieval and richer scoped APIs;
- live external research, secret-broker, MCP import, and trace/replay surfaces under P7 gates.
