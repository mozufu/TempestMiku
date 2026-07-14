# TODO

Last aligned: **2026-07-14**.

Active milestone: **P6 Android OS integrations — P6.2 share target closed**.

`ROADMAP.md` remains the canonical milestone order. P0-P5 are complete for the documented
single-owner deployment. P6 pairing, release hardening, provider-neutral push registration/outbox,
and authenticated Android notification actions have landed. The production UnifiedPush/ntfy path is
implemented, deployed on lumo, and proven by the physical killed-process request/resolution canary.
The bounded, review-before-send Android `text/plain` share target is also implemented and physically
proven; the next P6 OS-integration slice remains to be selected.

This file records the closed P7.1/P7.2a checklists and the explicitly deferred queue. Keep it aligned
with core docs §07, §08, §09, §10, §12 and product docs §21, §22, §26, §27, and §29. Closure is backed
by deterministic tests plus replayable public-surface evidence, not roadmap prose alone.

## Current Baseline

- [x] P4 dreaming produces durable, deduplicated `SkillProposalRecord` candidates with evidence,
      self-critique, verification checks, and source dream/session provenance.
- [x] P7.0 made skill proposals reviewable and auditable without live mutation; P7.1 now reuses that
      same approval/default-deny effect path for managed installation.
- [x] `write_proposal` and dream progress events persist in the replayable session event log.
- [x] Memory/profile writes, drive mutations, approvals, effects, and worker claims already have
      durable ownership and replay foundations.
- [x] Unknown capabilities and resource schemes fail closed; per-turn capability grants replace
      rather than accumulate authority.
- [x] The closed audit-hardening and native coding acceptance gates remain regression baselines.

## P6.1 UnifiedPush Production Delivery

- [x] Select UnifiedPush with a self-hosted ntfy distributor; Firebase/FCM remains disabled.
- [x] Add a production `unifiedpush` server provider that accepts only an explicitly configured HTTPS
      endpoint origin, refuses redirects, encrypts the routing-only payload with RFC 8291
      `aes128gcm`, and preserves transient/permanent retry semantics.
- [x] Register the Android app through the official UnifiedPush connector, send the encrypted
      endpoint/key envelope through the authenticated device API, and render request/resolution
      notifications from a killed app process without starting a second Flutter execution path.
- [x] Add the self-hosted ntfy service and public `push.justaslime.dev` Traefik route to `~/deployment-config`.
- [x] Deploy ntfy and the combined API/worker role on lumo with one persistent
      `TM_PUSH_ENCRYPTION_KEY`, `TM_PUSH_PROVIDER=unifiedpush`, and
      `TM_UNIFIED_PUSH_ENDPOINT_ORIGIN=https://push.justaslime.dev`. Loopback/public health,
      12 ordered migrations, restart persistence, and Hermes coexistence pass.
- [x] Install ntfy Android 1.24.0 from the official F-Droid release, configure
      `https://push.justaslime.dev`, install the signed TempestMiku release APK, pair it to lumo, and
      persist one active encrypted device registration without provider errors.
- [x] Prove approval request and resolution delivery on a physical Android 15 device while Flutter
      is killed. The request delivered at 2026-07-14 07:32:24 +08:00 and the timeout resolution at
      07:33:23, both in one attempt without provider errors; the native service created and then
      cancelled the notification while the Flutter activity stayed closed.

Closed-canary invariants:

- Do not treat the P6.1 closeout as completion of broader P6 OS integrations.
- Do not add Firebase SDKs, Firebase project configuration, or server credentials.
- Keep the provider disabled by default; fake-provider and encrypted local-provider tests remain the
  deterministic regression path.

## P6.2 Android Share Target

- [x] Export only an Android `ACTION_SEND` / `text/plain` target; reject unknown actions, MIME types,
      and empty content before it reaches Flutter.
- [x] Strip control characters, bound shared text to 16,384 characters and the optional subject to
      240 characters, carry an explicit truncation flag, and retain at most one pre-listener
      cold-start payload.
- [x] Bridge cold-start and singleTop intents through a client-only event channel without adding a
      Web behavior, server endpoint, capability, or second message loop.
- [x] Require an editable confirmation sheet. Never send on intent receipt; let the owner cancel or
      explicitly choose the current chat or a new chat.
- [x] Reuse `MikuSessionClient.sendMessage` for both destinations. New-chat imports create the
      session, persist the message, then load/attach so the durable turn remains recoverable across
      client lifecycle races.
- [x] Pure Kotlin parser tests cover accepted text, rejected action/type/empty input, sanitization,
      and bounds. Flutter tests cover event parsing, no-auto-send, editing, current-chat send, and
      new-chat routing.
- [x] Flutter analyze and all 44 Flutter tests pass; the focused Android app Kotlin test task passes;
      an arm64 debug APK builds successfully.
- [x] Build and verify a signed arm64 release APK with the established release identity, then install
      it in place over wireless ADB while preserving the paired app data and signer.
- [x] On a physical Android 15 device, expose TempestMiku in the system `text/plain` Sharesheet,
      import a URL into the editable preview, and prove cancellation does not send. Send distinct
      reviewed shares once to the current chat and once to a new chat; after force-stop/cold-start,
      both durable turns remain in their intended sessions with no preview replay or duplicate send.

P6.2 invariants:

- Do not accept files, images, HTML, arbitrary URI grants, pairing data, or non-`text/plain` MIME
  types in this slice.
- Do not auto-send shared content or bypass the normal authenticated session client.
- Shared content is untrusted user input; importing it grants no host capability or approval.

## P7.0 North Star

Make self-evolution authority explicit, attenuated, and explainable before any live catalog mutation:

- `self_evolution.tier` is parsed and enforced at config/registry boundaries, never only in prompts.
- The default conservative tier can reach only existing memory/profile and skill-proposal surfaces.
- Moderate mode may produce reviewable diffs but cannot apply them without durable approval.
- Aggressive authority remains unavailable in P7.0; configuration requesting it fails closed.
- Every attempted self-change has a durable audit record covering origin, target, tier decision,
  approval, effect, and replay outcome without storing secrets or unbounded content.
- Restart, retry, deny, timeout, and replay cannot duplicate or widen a write.

## Non-Negotiable Invariants

- The model still gets one chat-native tool: `execute(code)`.
- Streaming remains the source of truth; visible evolution activity uses the versioned durable
  `session_event` envelope.
- A tier is an upper authority bound, not an automatic grant. Exact capability grants and approval
  policy still apply inside the tier.
- Lower tiers cannot dispatch higher-tier effects through forged payloads, stale approvals, retries,
  or direct host calls.
- `SOUL.md` remains hand-owned. P7.0 never writes persona identity, mode definitions, prompts,
  capability configuration, source code, or deployment configuration.
- Approval deny/timeout/default-deny writes nothing. Unknown tier values, targets, effect kinds,
  capabilities, and resource schemes fail closed.
- Proposal and audit payloads are bounded and redact credentials, private keys, authorization data,
  and sensitive source content before persistence.
- No raw shell, ambient filesystem/network access, or second orchestration loop is introduced.
- Normal `cargo test` remains external-service-free. Postgres, live-provider, and physical-device
  checks stay explicitly gated.

## P7.0 Acceptance Gate

- [x] Changing configured tier changes reachable write targets at the registry/effect boundary.
- [x] Conservative mode accepts memory/profile and skill proposal flows while rejecting persona,
      mode, prompt, capability-config, source-code, and arbitrary-path targets.
- [x] Moderate mode persists a reviewable diff proposal but applies nothing before approval.
- [x] Aggressive mode is not executable in P7.0 and fails startup or dispatch with a stable error.
- [x] Every proposal attempt and resolution has a bounded audit record with actor/session/dream
      provenance, target class, tier decision, approval/effect ids, status, timestamps, and digest.
- [x] Deny, timeout, stale approval, forged target, retry, and restart tests prove no partial or
      duplicate write.
- [x] Public session-event replay explains the complete lifecycle without internal logs or full
      sensitive content.
- [x] SDK types, config examples, design docs, `ROADMAP.md`, and this checklist agree.
- [x] Strict Rust gates and a focused `tm-e2e` evolution-policy record pass before closeout.

## P7.0.1 Freeze Policy And Wire Contracts

- [x] Define `SelfEvolutionTier` with `off`, `conservative`, and `moderate`; reserve `aggressive` as a
      rejected value for this milestone.
- [x] Define target classes instead of path strings: `profile_fact`, `scoped_memory`,
      `skill_proposal`, `persona_proposal`, and `mode_proposal`.
- [x] Define the tier/target matrix in one policy module shared by config validation and effect
      dispatch.
- [x] Specify stable denial reasons for disabled tier, insufficient tier, unsupported aggressive
      mode, unknown target, approval required, stale approval, and invalid payload.
- [x] Define bounded proposal envelopes using resource references/digests for large bodies.
- [x] Add serde wire tests for tiers, targets, decisions, audit records, and forward-field handling.

Acceptance:

- [x] The contract is documented in §26 before callers depend on it.
- [x] Unknown enum values and arbitrary paths cannot deserialize into executable authority.

## P7.0.2 Configuration And Least-Authority Enforcement

- [x] Add `self_evolution.tier` with a fail-closed default compatible with current deployment;
      document migration from existing write-approval knobs.
- [x] Validate tier configuration at startup and expose only non-sensitive effective status through
      readiness/diagnostics.
- [x] Enforce target policy at the registry/effect boundary in `tm-server`/`tm-host`, not in prompts
      or client code.
- [x] Keep capability grants orthogonal: a permitted target still requires the exact registered
      capability and applicable approval.
- [x] Reject direct dispatch, forged proposal rows, and stale approved effects exceeding current
      tier.
- [x] Re-check authority immediately before applying an effect so lowering the tier revokes queued
      higher-tier work.

Acceptance:

- [x] Table-driven tests cover every tier/target pair and unknown values.
- [x] Restart tests prove lowering a tier prevents queued but unapplied higher-tier effects.

## P7.0.3 Durable Audit Trail

- [x] Add an append-only evolution audit record or extend current durable proposal/effect schema
      without creating a competing event source.
- [x] Record proposal origin, actor/session/dream ids, target class/id, content digest, configured
      tier, policy decision, approval id, effect id, status, timestamps, and stable error code.
- [x] Keep full candidate content behind capability-gated resources; events carry bounded previews
      and references only.
- [x] Redact before hashing/persistence when input may contain credentials or sensitive content.
- [x] Make attempt/resolution writes transactional with corresponding proposal/effect state.
- [x] Add idempotency keys so retries and completed-dream replay cannot create a second effect.
- [x] Add gated Postgres migration, upgrade/backfill, restart, and concurrent-resolution tests.

Acceptance:

- [x] An audit query explains allowed, denied, timed-out, superseded, failed, and applied attempts.
- [x] No audit/event payload contains raw secrets, unbounded diffs, or host paths.

## P7.0.4 Conservative Skill Lifecycle Boundary

- [x] Reuse the existing P4 `SkillProposalRecord`; do not add a parallel candidate format.
- [x] Add capability-gated read/list/preview for proposal resources before any install API.
- [x] Define skill identity, normalized name, version, provenance, digest, conflict, rollback, and
      catalog-reload contracts.
- [x] Validate `SKILL.md` structure, size, referenced paths, and prohibited authority before a
      proposal can become installable.
- [x] Keep installation and live reload disabled until audit, approval, atomic-write, and rollback
      tests pass in a later P7 slice.
- [x] Prove conservative mode cannot mutate hand-authored skills through `fs.*`, `code.*`, or a
      forged evolution effect.

Acceptance:

- [x] P7.0 reviews and audits a skill proposal end to end without changing the live catalog.
- [x] Existing P4 proposal behavior and deduplication remain backward compatible.

## P7.0.5 Moderate Review Surface

- [x] Extend the shared proposal model for persona/mode addenda as typed reviewable diffs, never raw
      arbitrary-file patches.
- [x] Require durable manual approval for every moderate proposal; no auto-approve in P7.0.
- [x] Surface bounded before/after metadata and resource links through existing approval/event paths.
- [x] Re-evaluate tier, target, base version, and digest after approval and before any future apply.
- [x] Keep apply disabled unless a later explicitly approved slice adds atomic versioned write and
      rollback support.

Acceptance:

- [x] Flutter/Web renders, approves, denies, times out, and replays the lifecycle without client-side
      policy logic.
- [x] Approval changes durable proposal state but cannot mutate persona or mode files in P7.0.

## P7.0.6 Verification And Closeout

- [x] Add `tm-e2e record evolution-policy` using scripted candidates and the public API.
- [x] Record conservative allow/deny, moderate approval-with-no-apply, timeout/default-deny, tier
      downgrade, forged-target rejection, retry idempotency, and replay continuity.
- [x] Verify bounded artifact/resource spill for large candidate bodies.
- [x] Run focused crate/server tests, then strict workspace gates.
- [x] Run gated Postgres migration/restart/concurrency coverage.
- [x] Run Flutter/Web smoke after event/resource payloads stabilize.
- [x] Sweep stale wording across §07, §08, §12, §22, §26, §27, §29, SDK types, config examples,
      `ROADMAP.md`, and `TODO.md`.
- [x] Mark P7.0 complete only after its evidence report and manifest exist under `target/tm-e2e`.

Verification commands:

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace
    TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=postgres://... cargo test -p tm-server
    cargo run -p tm-e2e -- record evolution-policy
    nix develop --command bash -lc 'cd clients/miku_flutter && flutter analyze && flutter test'
    cd clients/miku_web && npm test
    git diff --check

## P7.1 Managed Skill Lifecycle

- [x] Approved, verified `SkillProposalRecord` candidates install as immutable digest-addressed
      versions under the configured managed-skill root.
- [x] Bundled and hand-authored skill names cannot be replaced by managed versions; malformed names,
      bodies, digests, manifests, paths, and symlinks fail closed.
- [x] Activation uses an atomic pointer and `ModesConfig` loads the active version on the next prompt
      composition without process-global mutable state.
- [x] Trigger metadata is carried into the layered skill catalog so an installed skill is reachable
      only for matching turns.
- [x] `skill://`, `skill://<name>`, and version resources expose only installed managed skills and
      immutable bodies through both the authenticated resource gateway and the capability-gated
      native Deno registry; unconfigured catalogs remain an unknown resource scheme.
- [x] Rollback requires a durable manual approval, expected-current digest, target-version
      provenance, tier re-check, and atomic pointer swap.
- [x] Deny, timeout, stale pointer, hand-authored collision, tampered body, and retry paths cannot
      widen authority or mutate the active version.
- [x] Post-session approvals recover through `/sessions/:id/messages` `pendingEvents`; `session_end`
      remains the SSE terminal fence.
- [x] `tm-e2e record evolution-policy` proves two installs, immutable versioning, approval-backed
      rollback, public `skill://` reads, catalog reload, and durable lifecycle events.
- [x] Strict Rust workspace gates, gated Postgres coverage, Flutter/Web smoke, and documentation/SDK
      drift checks pass before closeout.

## P7.2a Managed Mode Addenda

- [x] Only Moderate `mode` proposals with `description` / `routing_guidance` sections expose
      `applyEnabled: true`; persona proposals remain review-only.
- [x] Approved proposals install immutable digest-addressed versions under a separately configured
      managed-mode-addendum root and atomically replace only the per-mode active pointer.
- [x] Apply rechecks tier, typed target, proposal digest, live base profile digest, and expected active
      pointer immediately before mutation; stale or forged effects fail closed.
- [x] The next prompt composition includes the approved guidance while the base `ModeProfile` remains
      unchanged for label, capabilities, voice cap, scope, skills, capability class, and route data.
- [x] Rollback uses a separate durable manual approval and can activate an older immutable version or
      restore the unmodified base catalog.
- [x] Deny, timeout, stale base/pointer, invalid sections, symlinks, duplicate resolution, and retry
      paths cannot mutate or widen authority.
- [x] `TM_MANAGED_MODE_ADDENDA_PATH` defaults beneath the artifact root and must be shared by API and
      worker roles, matching the managed-skill deployment contract.
- [x] Focused `tm-modes` / `tm-server` tests, Flutter parsing/rendering smoke, and
      `tm-e2e record evolution-policy` prove apply, replay, composition, invariance, and rollback.

## Explicitly Deferred Beyond P7.2a

- Applying moderate persona proposals or changing mode capabilities, voice caps, scopes, skills,
  route triggers, `SOUL.md`, or hand-authored `modes.json`.
- Any aggressive-tier execution or autonomous identity/config/source-code rewrite.
- `tm-mcp`, MCP import/reload gates, and a new external tool surface.
- `tm-trace` extraction unless audit implementation proves a second concrete consumer.
- Live external research, generic `http.*`, production egress allowlists, and the secret broker.
- Cloud drive sync, CRDT/conflict UX, pgvector/graph recall, and LLM-backed extraction.
- Firebase/FCM integration; UnifiedPush/ntfy is the selected production path.

## Ownership

- `tm-memory`: candidate production, refinement, deduplication, and provenance.
- `tm-server`: config validation, tier/effect enforcement, durable audit/proposal state, approvals,
  transactions, replay events, migrations, and public resource routes.
- `tm-host`: capability registry and target dispatch boundary where host authority is exercised.
- `tm-modes`: typed persona/mode proposal targets plus the managed immutable skill catalog; it never
  mutates bundled or hand-authored assets.
- `tm-sandbox`: checked-in SDK/docs exposure and denial coverage; no extra model-visible tool.
- Flutter/Web clients: render server-owned proposal/approval/resource contracts only.
- `tm-e2e`: public-API evidence for policy, approval, restart, idempotency, and replay behavior.
