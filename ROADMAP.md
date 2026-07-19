# TempestMiku roadmap

This is the canonical roadmap for TempestMiku. Design-doc section references remain stable:

- Core runtime roadmap: §14.
- Product roadmap and execution order: §28.

TempestMiku is a **rewrite** (§29): reach behavioral parity with the current `hermes-agent`, then deepen.
Dogfood in this order: **coding agent → project manager → personal assistant**. The first useful slice must help build TempestMiku itself, then manage the project around that work, then broaden into the full companion.

## Core runtime milestones (§14)

| Milestone | Deliverable |
|---|---|
| **M0 — streaming protocol skeleton** ✓ | Streaming-first OpenAI client (SSE) + the agent loop consuming deltas live + an `EventSink` (assistant tokens token-by-token). The original test-only stub was retired after the tm-lang-only cut. |
| **M1 — real REPL** ✓ | Persistent tm-lang runtime; `display`, allowlisted `http.get`, artifacts/resources, result shaping, and spill. The original Deno/V8 implementation is historical and no longer ships. |
| **M2 — disclosure + tools** ✓ | `tools.search/docs/call`; the catalog/SDK translation contract for external tools. `tools.call` re-enters the registered host boundary with the target capability rechecked, and selected production `tm-mcp` imports ship through the closed P9 boundary. |
| **M3 — production loop** ✓ | Approval gates, audit log, session pooling, deterministic replay. (Streaming landed in M0.) |
| **M4 — runtime hardening** ✓ | P9 egress hardening and both opt-in Linux `proc.run` profiles are implemented and canaried. `linux_hardened_v1` adds a fixed repo-owned seccomp policy plus per-run cgroup-v2 CPU/memory/pids enforcement and crash-orphan recovery without fallback. Disposable native aarch64/x86_64 executions and the selected homolab production service pass. The retained native x86_64 report proves exact systemd delegation, cgroup exclusivity, representative workload sizing/headroom, immutable identities, persistence, and restart stability under the selected hostile-workload/trusted-host-kernel boundary. Hostile host-kernel containment and microVM isolation are not claimed. |

## Product roadmap (§28)

| Stage | Deliverable | Modes (§21) | Needs (core) | Acceptance (incl. parity §29.5) |
|---|---|---|---|---|
| **P0a — OMP ACP bridge** ✓ | Transitional Serious Engineer backend: `tm-server` delegates coding execution to a pinned `omp acp` process, maps ACP progress/diffs/finals into TempestMiku SSE + event log, and routes ACP permission requests through TempestMiku approvals while Miku owns persona/final voice | 4/5 | M0 + `tm-server` | real TempestMiku patch edit + targeted test succeeds through OMP ACP; progress replays via `Last-Event-ID`; at least one permission path is resolved by TempestMiku approval UI/API; output is mirrored to `artifact://`/resource refs with provenance; serious voice cap is `off`; OMP remains a replaceable backend, not the final SDK surface |
| **P0 — coding-agent dogfood** ✓ | Serious Engineer slice for TempestMiku itself: `fs.*`/`code.*`/`proc.*` (§25), linked repo, artifacts, minimal project memory, streaming CLI | 4 | M0–M2 | real linked-repo patch edit + targeted test succeeds; output spills to `artifact://`; project summary/open-loop memory is recalled next session; approvals gate destructive/out-of-grant ops; serious voice cap is `off` |
| **P1 — project manager + remote control** ✓ | Chief-of-staff layer over the coding loop: mode router, project plans/open loops/decisions, session event log, SSE/API project surface, project promotion, and Flutter Web/PWA remote-control client | 1/2/4 | M1–M2 | router fires; user can lock; badge + voice cap work; project status survives reconnect; Miku can turn a coding session into next actions without losing provenance; a phone/browser can attach to the same session stream, send a message, resolve an approval, open resource links, promote session workspace/artifacts into `project://`, and resume via `Last-Event-ID` |
| **P2 — personal assistant** ✓ | Full companion baseline: Miku voice, profile/user memory, bounded every-third-turn dialectic, personal-assistant state capture, negative-state grounding, bounded proactivity | 1–3 | M1–M2 | bounded dialectic, memory context/profile, approval-backed capture, and the frozen General/grounding/Serious rubric are test-backed; a retained opt-in `gpt-5.5` non-echo public-API run passes all actual-output voice criteria |
| **P3 — handoff + orchestration** ✓ | P3 MVP `agents.run/spawn/parallel/msg` (§23): actor sessions + message passing + supervision; Handoff mode + brief template; actor lifecycle events in SSE stream | 5 | M2 | ✓ fan-out N workers, only digest returns to parent context; ✓ siblings coordinate via one-shot msg; ✓ crashing sub-agent isolated + `actor_failed` in SSE; ✓ handoff matches `oh-my-pi-handoff` |
| **P3+ — live actor mailbox** ✓ | Live MPSC resident delivery (`agents.send/wait/inbox/list/broadcast/cancel/pipeline`); active supervision (restart/subtree cancel); child approval routing through `ApprovalBroker`; DAG acyclicity + protocol invariants; Flutter smoke coverage | 5 | M2 | live siblings coordinate via `send`/`wait`; child cancel yields replayable `Cancelled` event; child approval requests reach the user; restart/timeout/subtree-failure decisions are replayable; Flutter attach/approve/reconnect pass |
| **P4 — dreaming + proactivity** ✓ | consolidation + `skills/` generation (§22/§26); scheduler/cron (§27.2) | all | — | transactional session end, durable approvals/effects, fenced leases, enforced cron bounds, supervised roles, recovery tests, and final client verification pass |
| **P5 — drive + research** ✓ | `drive.*` + auto-organizer (§24): transducers + virtual dirs; deep-research workspace (P3+P4+P5) | 4/5 | — | Postgres metadata/organizer/link persistence, CAS application, tombstones, startup link revalidation, and final restart/client verification pass |
| **P6 — Android package + OS integrations** ✓ | Shipped actionable notifications, bounded quick capture, and editable voice drafts with local-default plus an explicit owner-controlled home-ASR option | all | — | P6.1-P6.5 retain their signed Android acceptance. P6.6 deterministic software, host inference, signed packaging, exact model activation, airplane-mode cold start, same-device synthetic A/B, lifecycle/cleanup, healthy and unavailable self-hosted paths, exact-once current/new sends, and the consented production-local 10-item speaker corpus all pass. Local and self-hosted modes never silently fall back or auto-send and both require editable review. |
| **P7 — self-evolution tiers + hardening** ✓ | Typed, attenuated, reviewable evolution (§26), including P7.2b approval-backed persona addenda | all | M3 + relevant shipped M4 boundaries | Auto mode can create only bounded evidence-backed persona proposals; durable manual approval/rollback is the sole activation path and cannot mutate identity, authority, safety, modes, or `SOUL.md` |
| **P8 — fuller memory** ✓ | Self-hosted hybrid lexical/dense recall, scoped episodic/semantic stores, evidence-backed extraction, correction, and measured recall quality (§22) | all | — | Frozen overall/held-out nDCG and Recall gates pass with zero unsafe inclusions; hybrid turn injection, exact retry/replay resources, provider-loss fallback, resumable re-embedding, restart, strict Postgres/client gates, and lumo self-hosting evidence are closed |
| **P9 — production egress + secret broker** ✓ | Destination-scoped audited egress and opaque secret handles at the host boundary (§07/§08) | 4/5 | M4 egress boundary (shipped) | exact destination/DNS/IP/redirect policy, durable cross-instance budgets and mutation receipts, revocation, denial, restart, audit, and live HTTPS gates pass; production egress remains disabled by default |
| **P10 — MCP + live research** ✓ | Import selected MCP capabilities into lazy SDK/resource surfaces and run live research through P9 authority (§25) | 4/5 | M2 + P9 | one model-visible `execute(code)` tool remains; exact catalog, strict schemas/results, prompt-injection isolation, provenance, durable approval-backed writes, audit/replay, Streamable HTTP, and an official read-only live canary pass |

**Parity gate (§29.5):** P0–P4 are not "done" until they reproduce the current behavior for their
slice (coding reach, project continuity, voice, mode router, memory recall, approvals). New capability layers on *after* parity.

## Execution plan from current workspace (2026-07-18)

The owner-priority **tm-lang runtime** described by §tm and `TODO.md` is closed. Its frozen
conformance, checker, persistence, effect/approval, Postgres/replay, affected-client, and 20-prompt x
50-run historical fluency gates passed on 2026-07-16. The subsequent hard cut made tm-lang the only
language/runtime: Deno/V8, the shipped stub, backend selectors, and the executable comparative
runner were removed without rewriting old durable events. The 2026-07-18 completion pass then
implemented M2 `tools.call`, P7.2b, P9, P10, both executable M4 Linux profiles, and the P6.6
production path in dependency order. It did not collapse acceptance boundaries: an early consented
recording after the immutable-buffer fix failed quality, so P6.6 stayed open until the later signed
`1.0.3+4` lifecycle/failure/current/new matrix and consented 10-item local corpus passed on
2026-07-19. M4's seccomp/cgroup software profile passes disposable Linux/aarch64 and native
x86_64 canaries. The owner then selected homolab under the hostile-workload/trusted-host-kernel
boundary; its persistent NixOS service, exact delegation, representative sizing, cgroup exclusivity,
native report, and restart gates pass. Hostile host-kernel containment and microVM isolation remain
explicitly unclaimed rather than being prerequisites of the selected contract.

Current implementation has advanced past the original M0-only substrate. The workspace now includes
`tm-core`, `tm-llm`, `tm-lang`, `tm-artifacts`, `tm-host`, `tm-modes`, `tm-agents`, `tm-server`,
`apps/tm-cli`, and client scaffolds under
`clients/`. The implemented path covers M0, M1, host/approval/resource foundations, P0a OMP ACP
bridging, the native P0 Serious Engineer dogfood slice, the CLI native cutover proof for
linked-repo edit/run/artifact workflows, native tm HTTP approvals for the Serious Engineer backend,
the P1 project-manager remote-control surface, the P2 personal-assistant baseline, the full P3
actor orchestration surface, and the P4 dreaming/scheduler slice. P3 ships `agents.run/spawn/parallel/msg`, the `agent://` + `history://`
resource handlers, child artifact spill + transcript history, actor lifecycle events in the SSE
stream, Handoff mode wired from the mode catalog, and `CapabilityGrants` glob permit support. P3+
is also closed: bounded per-actor inbox queues, `Agent::run` inbox draining,
`agents.send/broadcast/wait/inbox/list/cancel/pipeline`, delivered-message and status lifecycle
events, actor-id threading through `InvocationCtx`, child approvals routed through the live
`HttpApprovalPolicy` / `ApprovalBroker`, parent-session child artifacts, parent-driven actor cancel
tokens with replayable `actor_cancelled` events, active restart supervision (`one_for_one`,
`one_for_all`, `rest_for_one`), wall-clock actor timeouts, fail-fast sibling-subtree supervision,
typed parent `SessionEvent` resource links, `actor_resources_linked` provenance events, and
tm-e2e/Flutter actor smoke coverage. P4's test-backed mechanisms include dream queue records,
deterministic dream extraction,
redaction/config guardrails, session/reflection/rollup summaries, approval-gated memory and skill
proposals, Postgres FTS recall coverage, cron job/run helpers, and Flutter/Web smoke coverage for
dream-origin proposals and replay. P5's test-backed mechanisms include `tm-drive`, `drive.*`,
local-first
blob-backed entries, transducers, search/virtual-dir feeds, approval-gated dropped-file writes,
shared `write_proposal` organizer events, `drive://` resource resolution, replayable drive mutation
events, project links that mint `FsPolicy` and memory scopes, and local research fan-out over drive
docs with per-child budgets are covered by deterministic Rust/e2e/client tests plus gated Postgres
schema/resource/replay tests.
The software integration command matrix passes, including clean-schema Postgres, split API/worker
restart, authenticated Playwright, Flutter, signed arm64 Android APKs, certificate, RustSec, and
secret scans. P6.6's separate signed-device and consented-speaker gates also pass; software tests
were not used as a substitute for them.
A physical Android 15 release canary over Tailscale Serve HTTPS also proves in-app QR confirmation,
durable chat, cold-start credential/session recovery, exact-once replay of a turn completed while the
app was stopped, and immediate SSE/API loss after device revocation. Managed sandboxes may need
elevated permissions for tests that bind `127.0.0.1`.

The audit hardening implementation is now wired. Postgres upgrades through ordered checksummed
`schema_migrations`; session end and dream enqueue are transactional; device credentials, turns,
approval requests/effects, dream/cron leases, and drive metadata/link tombstones are durable. Runtime
roles are explicit: `api` serves HTTP, `worker` dispatches turns/effects/dreams/cron, and `all` does
both. Worker roles require Postgres and drain under a supervised shutdown grace. The public API uses
server-owned owner/scope authority, `202` idempotent turns, and one durable `session_event` SSE
envelope. Android/Web use the same device records through bearer tokens or an HttpOnly cookie.

The audit hardening gate is closed for the documented single-owner deployment. Production exposure
remains loopback-only behind an HTTPS reverse proxy or Tailscale Serve, and Postgres remains
mandatory outside loopback and for embedded workers. This gate does not broaden the deployment
target into multi-tenancy or arbitrary public-internet hosting.

P6.6 and full P6 are closed with signed Android inference/lifecycle/resource and consented-speech
acceptance. M4 is closed for its selected homolab hostile-workload/trusted-host-kernel contract;
hostile-kernel containment and a microVM are not claimed. P7.2b, P9,
and P10 are software-complete with focused, real-Postgres, replay, and live read-only evidence.
Optional cloud drive sync, `code.ast`/`code.lsp`, `tm-trace`, generated SDK docs, and any future
second sandbox backend are
demand-triggered rather than part of the committed critical path.

Sizing below is **solo focused engineering days**. Treat it as sequencing pressure, not a promise:
each milestone is done only when its acceptance checks pass.

| Order | Milestone | Estimate | Tasks | Acceptance |
|---:|---|---:|---|---|
| 0 | **DONE — M0 closeout: streaming skeleton locked** | 0.5–1d | Freeze stream event contract; keep OpenAI SSE adapter tests and CLI token streaming. The original stub proof was later retired. | `cargo test`; scripted stream tests prove token deltas, tool-call assembly, execution, and final answer path. |
| 1 | **DONE — M1 real REPL + SDK spine** | 4–7d | Ship the persistent REPL, cancel/reset, `display`, `http.get` allowlist, artifacts, `CellBudget`, and output caps. The first Deno implementation was later replaced and deleted by tm-lang. | Rust tests cover tm cells, persistence, reset, display/print capture, blocked ambient I/O, timeout/cancel, shaped output truncation, and artifact spill/readback. |
| 2 | **DONE — host registry + approvals foundation** | 3–5d | Add `tm-host`; define `HostFn`, capability grants, `ResourceRegistry`, `ApprovalPolicy`; wire SDK dispatch from sandbox ops; fail closed on unknown schemes/capabilities. | Unit tests prove denied capability cannot execute, approval timeout denies by default, `artifact://` resolves through the registry, linked paths cannot escape roots, and unsafe operations gate or fail closed. |
| 2.5 | **DONE — P0a OMP ACP bridge path** | 2–4d | Add an OMP ACP coding backend behind `tm-server`: spawn/connect a pinned `omp acp` process with generated linked-repo config, translate ACP session/progress/diff/final/permission messages into `session_events` + SSE, mirror outputs into artifacts/resource refs, and keep final user-facing response under Miku persona. | Tests cover ACP event normalization, diff/artifact events, generated approval mode, permission request round trips, approval route resolution, and replay from `Last-Event-ID`. The retained `omp/16.4.8` live dogfood used a real model, resolved one public-API approval, made the exact source edit, ran the targeted test, emitted artifact/final events, and replayed through `Last-Event-ID` (`1 passed`, `0 failed`, 31.56 s); see [`P0a live evidence`](docs/evidence/2026-07-18-p0a-omp-acp-live.json). |
| 3 | **DONE — P0 coding-agent dogfood first pass** | 5–8d | Add config-declared linked-folder `FsPolicy`; implement explicit `fs.write/patch/move/remove`, search, argv-only `proc.run(cmd,args)`, artifact spill, minimal project memory, and a streaming CLI surface for Serious Engineer mode. | Tests prove linked-folder read/write/list/find, atomic patch plus stale-tag rejection and bounded diff spill, approval-gated overwrite/remove behavior, argv-vector `proc.run`, shell-string rejection, non-allowlisted denial, output spill, CLI tm host effects, native tm approve/deny/timeout through the HTTP approval route, serious-mode voice cap, and project summary/open-loop recall. |
| 4 | **DONE — P1 project manager + remote control** | 4–7d | Added the project-manager layer over the existing server: mode router lock/override, `ModeChanged` events, project/open-loop/decision views, session-to-project promotion, fuller session-scoped resource gateway, and Flutter Web/PWA remote-control polish. | Tests and smoke coverage prove reconnect replay from event id, router lock/unlock, project status and decisions across sessions, `POST /sessions/:id/promote` to `project://` resources with provenance, approval/resource workflows, and Flutter Web/PWA attach/send/watch/resume behavior. |
| 5 | **DONE — P2 personal assistant baseline** | 5–8d | Added the companion baseline: full persona overlay via SOUL + mode skill bundles, profile/user recall, bounded every-third-turn dialectic with durable replay, state capture, negative-state grounding, and approval-backed reminder/open-loop capture. | Deterministic tests cover dialectic cadence/caps/off modes/replay and a positive/negative General/grounding/Serious rubric. A retained real non-echo `gpt-5.5` `tm-e2e voice-eval` report passes every General, grounding, and Serious criterion; see [`P2 live voice evidence`](docs/evidence/2026-07-18-p2-voice-live.json). |
| 6 | **DONE — P3: handoff + sub-agent actors** | 6–10d | Added `tm-agents` with actor lifecycle, mailbox, roster, `agents.run/spawn/parallel/msg`, `agent://` + `history://` resources, child artifact spill + transcript history, actor lifecycle events in SSE stream, supervision defaults, Handoff mode wired from catalog, `CapabilityGrants` glob permit. | ✓ Fan-out N workers, only digest returns to parent context; ✓ siblings coordinate via one-shot msg; ✓ child crash isolated + `actor_failed` in SSE log; ✓ handoff matches `oh-my-pi-handoff`. |
| 6+ | **DONE — P3+ live actor mailbox + supervision** | 4–8d | Landed per-actor bounded inbox + `Agent::run` inbox draining, `agents.send/broadcast/wait/inbox/list/cancel/pipeline`, child approval routing through the live broker, parent-session child artifacts, parent-driven cancel tokens with replayable `actor_cancelled`, plain-prose enforcement, live-wait DAG acyclicity checks, pipeline digest-reference wiring, active restart strategies (`one_for_one`, `one_for_all`, `rest_for_one`), wall-clock budget enforcement, fail-fast sibling-subtree supervision, status lifecycle events, typed parent `SessionEvent` actor/artifact/history links, and tm-e2e/Flutter actor smoke coverage. | Tests prove live sibling coordination, cancel/replay, child approval routing, restart decisions, actor timeouts, fail-fast sibling-subtree cancellation, event-log resource provenance, and Flutter attach/approve/reconnect smoke coverage. |
| 7 | **DONE — P4 dreaming + scheduler production hardening** | 6–10d | Added transactional lifecycle writes, durable approval requests/effects, fenced owner/epoch leases with heartbeat/retry limits, atomic cron cursor materialization, deny-only execution bounds, supervised worker roles, and recovery metrics. | Deterministic, gated, split-process, and physical-client verification cover stale-owner rejection, bounded retries, transactional enqueue, restart recovery, and authenticated replay. |
| 8 | **DONE — P5 drive + research workspace production hardening** | 5–9d | Added `DriveMetadataStore`, Postgres entries/proposals/organizer/corrections/version/link/tombstone state, CAS moves/apply, startup canonical-root revalidation, and the documented no-database historical metadata exception. | Deterministic, gated, split-process, and physical-client verification cover persistence, revocation, hydration, concurrent application, and restart-safe access. |
| 9 | **DONE — P6.1 UNIFIEDPUSH PRODUCTION DELIVERY** | 4–7d | The provider-neutral foundation has a production UnifiedPush adapter with exact-origin/redirect policy and RFC 8291 payload encryption. Android uses the official connector, keeps notification handling native while Flutter is killed, and registers its endpoint through device auth. Self-hosted ntfy plus the combined API/worker role are live on lumo behind Traefik; Firebase remains absent. | Deterministic encrypted-provider tests, Flutter tests/analyze, a signed arm64 APK, the live lumo health/migration/restart gate, and the physical Android 15 request/resolution canary pass. Both delivery rows completed in one attempt without provider errors and native notification cancellation left the Flutter activity closed. |
| 10 | **DONE — P7.0 safety foundation + P7.1 managed skills** | 6–12d | P7.0 landed typed tiers/targets, least-authority effects, audit history, bounded replay, and Moderate review-only addenda. P7.1 adds approval-backed immutable managed skill versions, cross-process locked atomic activation/rollback, trigger-aware reload, and capability-gated `skill://` reads without allowing bundled/hand-authored replacement. Persona/mode apply, aggressive writes, MCP, and egress remain disabled. | Strict Rust/workspace, gated Postgres, Flutter/Web, and drift gates pass. Focused tests plus final `evolution-policy` evidence prove install, second-version activation, post-session approval recovery, rollback, reload, collision/tamper denial, and public resource reads. |
| 11 | **DONE — P7.2a managed mode addenda** | 2–4d | Moderate mode proposals install immutable description/routing-guidance versions behind manual approval, atomically activate/rollback under a cross-process lock, and compose on the next prompt without mutating `SOUL.md`, `modes.json`, capabilities, voice caps, scopes, skills, or route triggers. Persona apply remains disabled. | Focused crate/server tests and `tm-e2e record evolution-policy` prove apply, deny/timeout, stale-base denial, replay, next-turn composition, capability invariance, and rollback to the base catalog. |
| 12 | **DONE — P6.2 ANDROID SHARE TARGET** | 1–2d | The exported Android `ACTION_SEND` target accepts only `text/plain`, sanitizes and bounds text/subject input, and forwards cold-start or singleTop intents through a platform event bridge. Flutter requires an editable confirmation sheet and explicit current/new-session selection before reusing the durable message path. | Pure Kotlin parser tests and Flutter parser/widget tests cover rejection, bounds, no-auto-send, editing, and both destinations. Flutter analyze/full tests, Kotlin parser tests, signed arm64 APK verification, and the physical Android 15 Sharesheet/current/new-session cold-start canary pass. |
| 13 | **DONE — P6.3 ANDROID SELECTED-TEXT ACTION** | 1–2d | Add `ACTION_PROCESS_TEXT` / `text/plain` to the existing singleTop Android activity, accept only bounded `EXTRA_PROCESS_TEXT`, and reuse the P6.2 source-tagged event bridge plus editable current/new-session review flow. It never returns replacement text, consumes URI authority, or sends automatically. | Kotlin and Flutter tests, analyze, merged-manifest/APK inspection, and a Keychain-backed signed arm64 build pass. The in-place Android 15 canary proves OS resolver discovery, cancel without send, distinct current/new-session sends, warm delivery, and cold-start recovery without preview replay or duplicate user messages. |
| 14 | **DONE — P6.4 ACTIONABLE ANDROID NOTIFICATIONS** | 2–4d | Deep-link notifications to their exact session/approval context and accept a bounded inline reply only after the owner explicitly presses Send. Native code posts one idempotent message through the existing authenticated durable API; it never runs the agent loop. `MessagingStyle` keeps the direct-reply action visible on HyperOS/MIUI. | Local Kotlin/Flutter/server and gated Postgres tests prove routing, bounds, idempotency, retry classification, revocation, expiry, missing-session handling, and no duplicate turn. A Keychain-backed signed arm64 APK installed in place on Android 15 and proved foreground/background/killed exact-context taps, distinct and double-tapped exact-once replies, offline retry, cold-start recovery, and visible no-send behavior for empty/cancelled/oversized, expired, revoked, deleted-session, and ended-session cases. |
| 15 | **DONE — P6.5 QUICK CAPTURE** | 2–4d | One versioned native action accepts only a fresh UUID plus optional sanitized bounded text and rejects MIME, data, `ClipData`, selectors, streams, URI grants, and unknown extras. A static launcher shortcut passes through a no-history trampoline and the Quick Settings `TileService` uses that same intent; both land in the existing editable current/new-session review path. No widget or native send/model path was added. | Kotlin parser/buffer tests, Flutter parser/widget tests, analyze/full Flutter, strict Rust workspace gates, signed arm64 APK inspection, and in-place Android 15 proof pass. The real launcher menu and HyperOS tile opened from foreground/background/killed states; empty/cancel sent nothing, current/new confirmations produced exactly one turn each, duplicate event ids were ignored, and process death did not replay a draft. |
| — | **DONE — P6.6 VOICE CAPTURE + P6 CLOSEOUT** | 5–9d | The owner resumed the slice on 2026-07-18. Production has foreground-only bounded PCM capture, an explicit verified local model installer, local sherpa-onnx inference, on-device Traditional-Chinese conversion, and an optional fixed-destination home-ASR broker selected explicitly in the drawer. Recorder handling, aggregate signal diagnostics, buffer cleanup, editable review, and explicit current/new-session send remain fail closed; neither engine automatically falls back or sends. | Deterministic Flutter/Kotlin/Rust tests, production-inference host reproduction, signed arm64 packaging, latest signed-app install/selector, same-device synthetic baseline/candidate A/B, exact-reference self-hosted review/cancel, background/process-death cleanup, stopped-upstream no-fallback/no-send, distinct exact-once current/new sends, and the consented 10-item production-local corpus pass. Retained text-only metrics: mean CER `0.132922`, p90 `0.241379`, code-switch mean `0.281980`, with zero empty/truncated/signal-warning items. |
| 16 | **DONE — P8 FULLER MEMORY** | 8–14d | P8.1 froze replayable global/project lexical controls and contracts; P8.2 added restart-safe scoped records, evidence, corrections, tombstones, and jobs; P8.3 added local embeddings, pgvector/HNSW generations, and deterministic FTS+dense RRF; P8.4/P8.5 compose bounded results into durable turns/resources, type approved extraction, and close quality/deployment evidence. | Frozen overall and held-out quality improve beyond threshold with no Recall regression and zero unsafe inclusions. Exact retry/replay, scope/unlink/correction, provider-loss/restart, resumable re-embedding, strict Rust/Postgres/client gates, and self-hosted lumo evidence pass. |
| 16.5 | **DONE — tm-conformance-v2 BATCH HARDENING** | 1–2d | Hard-cut standalone `---` separators to semicolon cells and replace speculative unbound-name retries with scope-aware forward binding dependencies over one pre-batch snapshot. | V2 corpus, parser diagnostics, rebind/closure/interpolation chains, independent overlap, response-order collision, failed-producer no-effect, public API/e2e, and strict workspace gates pass; v1 remains historical evidence only. |
| 17 | **DONE — P7.2b APPROVAL-BACKED PERSONA ADDENDA** | 4–7d | The manual lifecycle adds immutable tone/address/interaction guidance, next-turn composition, and approval-backed rollback. Auto mode detects only repeated P8-backed preference/persona mismatch and creates typed, bounded, deduplicated, cooldown-bound proposals. | In-memory fault injection and real-Postgres trigger/two-instance/restart tests prove atomic proposal+approval/event creation, filesystem-before-finalization retry, auto-propose but never auto-apply, stable-base checks, deny/timeout/stale/retry safety, next-turn composition, rollback, and identity/authority invariance. |
| 18 | **DONE — P9 PRODUCTION EGRESS + SECRET BROKER** | 6–10d | Added exact destination allowlists, DNS/IP/redirect policy, durable request/byte/time budgets, audit/revocation, exact grants, durable mutation intents, and opaque destination-scoped secret handles resolved only at the host boundary. | Deterministic and real-Postgres gates cover denial, timeout, redirect/rebinding, query/body digest approval, cross-instance budget serialization, exact-once/uncertain replay, narrow literal redaction, restart/revocation, audit ordering, and a live public-HTTPS canary; egress remains disabled by default. |
| 19 | **DONE — P10 MCP + LIVE RESEARCH** | 6–10d | Added selected MCP resources/prompts/tools as lazy SDK/resource surfaces behind P9 while preserving `execute(code)` as the only model-visible tool. | Exact protocol negotiation, paginated atomic catalog activation, strict bounded schemas/results, prompt-injection isolation, untrusted provenance, durable approval-backed mutation, restart replay, Streamable HTTP JSON/SSE/resume, resource authority, and the official Cloudflare documentation MCP read-only canary pass. |

P6.6 was deferred on 2026-07-15 after its isolated feasibility and Taiwan Mandarin runs, then
explicitly resumed by the whole-roadmap request on 2026-07-18. The historical findings remain in the
[`P6.6 deferment evidence`](docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md); the implemented
production boundary, selected model, host reproduction, signed package, physical matrix, and
consented-speech closeout are recorded in the
[`P6.6 resumption evidence`](docs/evidence/2026-07-18-p6-6-on-device-asr-resumption.md), with the
privacy-stripped aggregate score in the retained
[`real-speaker evaluation`](docs/evidence/2026-07-19-p6-6-real-speaker-eval.json). P6.6 and full P6
are closed; the earlier failed attempt remains historical evidence rather than being rewritten.

M4 has two explicit opt-in Linux profiles. The original `linux_bubblewrap` profile closes the
namespace/rlimit and descriptor-pinning software gate. `linux_hardened_v1` additionally supplies a
sealed architecture-specific `developer_v1` seccomp program, a dedicated delegated cgroup-v2 leaf
per execution, bounded CPU/memory/pids controls, kill-and-drain cleanup for success/timeout/cancel,
and startup orphan recovery. Its disposable Linux/aarch64 and native x86_64 canaries pass, with no
fallback when the policy, delegated subtree, or pinned roots are unavailable. Exact policy digests, canary scope, and
the disposable boundary are recorded in the
[`M4 hardened Linux evidence`](docs/evidence/2026-07-18-m4-linux-hardened-v1.md). The owner-selected
homolab service then closed deployment acceptance: its persistent NixOS generation, dedicated
identity, systemd delegation, cgroup exclusivity, native selected-architecture execution,
representative memory/pids/CPU headroom, and restart stability pass in the retained
[`M4 homolab production evidence`](docs/evidence/2026-07-19-m4-production-homolab.md). The accepted
profile covers a hostile workload on a trusted host kernel. Hostile host-kernel containment and
microVM isolation are intentionally not claimed.

P8.1 closed on 2026-07-15 before ranking changes. Its nine-case clean-schema PostgreSQL 16 replay
freezes overall nDCG@5 `0.279754`, Recall@5 `0.444444`, held-out nDCG@5 `0.471713`, held-out
Recall@5 `0.75`, the existing 1,600-token/five-item bounds, and a 50 ms per-case p95 ceiling. The
versioned record/evidence and embedding-provenance contracts compile through both stores; the
machine-readable baseline and limitations are recorded in the
[`P8.1 recall baseline evidence`](docs/evidence/2026-07-15-p8-1-recall-baseline.md). P8.2 closed on
the same date with ordered scoped tables, legacy mirroring, correction/unlink denial, and readiness;
its storage-only contract and gates are recorded in the
[`P8.2 durable memory evidence`](docs/evidence/2026-07-15-p8-2-durable-memory-spine.md). P8.3 then
closed with a local-only batch embedding boundary, pgvector/HNSW generation tables, resumable leased
jobs, an atomic active-version pointer, and deterministic FTS+dense RRF that preserves evidence and
authority. Missing pgvector, disabled/provider-loss, partial/stale re-embedding, and dimension/model
mismatch keep lexical-only results visible; its exact gates are in
[`P8.3 local embedding evidence`](docs/evidence/2026-07-15-p8-3-local-embeddings-hybrid.md). P8.4/P8.5
then closed hybrid turn integration, exact bounded inspection, approved typed extraction, incremental
and resumable embedding, and the frozen quality/deployment gates. The historical frozen closeout
improved overall nDCG@5 from `0.279754` to `1.0` and held-out nDCG@5 from `0.471713` to `1.0`, with
no Recall@5 regression and zero unsafe inclusions. The later review-hardening replay with balanced
2/2/1 prompt allocation recorded overall/held-out nDCG@5 of `0.813059`/`0.721713`, Recall@5 of
`1.0`/`1.0`, and zero unsafe inclusions; it still passes the frozen baseline-relative contract.
The local lumo provider, provider-loss/restart, server restart, and exact gate matrix are recorded in
[`P8 fuller-memory closeout evidence`](docs/evidence/2026-07-15-p8-5-fuller-memory.md).
The follow-up PR review hardening adds serialized scope revocation and approved writes, monotonic
scope-revision-pinned embedding generations, exact authority-first dense queries, balanced 2/2/1
prompt allocation, and isolated oversized-input recovery without reopening P8 or changing the frozen
P8.1 control. The owner-priority tm-lang runtime, P7.2b, P9, and P10 are closed.

### 2026-07-19 integrated verification matrix

| Gate | Result | Claim boundary |
|---|---|---|
| `cargo fmt --all --check` | PASS | Final integrated Rust source |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | PASS | Strict full-workspace lint |
| `cargo test --workspace --all-features --locked --no-fail-fast` | PASS | Includes `tm-server` 327/327 library + 3/3 binary and `tm-host` 79/79 |
| RustSec `cargo audit --deny warnings` / `actionlint` | PASS / PASS | 357 locked dependencies checked against 1,166 loaded advisories; workflow syntax clean |
| Pinned Gitleaks history / current worktree | PASS / PASS | 293 commits / 15.24 MB and tracked-plus-untracked 8.40 MB scanned; no leaks found |
| Clean-schema PostgreSQL 16 + pgvector `gated_postgres` | PASS / 30/30 | Serial all-feature durability, replay, authority, P8, P9, P10, and persona tests |
| Flutter analyze / full suite | PASS / 113/113 | Local/default plus explicit self-hosted voice draft, authority-transition cancellation/epoch fencing, aggregate diagnostics, installed-build fingerprint, and current/new-session source |
| Android app unit tests | PASS / 29/29 | Capture, model installer, immutable-buffer, and installed-APK fingerprint regressions included |
| Android ASR benchmark harness | PASS / 20/20 | Analyze clean; host verifier independently recomputes schema-3 metrics/gates, binds local corpus/APK digests, and emits a same-installation A/B report |
| Flutter Web release / Playwright | PASS / 1 passed, 1 intentionally gated skip | Standard chat smoke passed in 26.4 s; the evidence-only test requires its separate opt-in environment |
| P0a real OMP ACP dogfood | PASS / 1/1 | `omp/16.4.8`, real model, public approval, edit/test/artifact/final/replay, 31.56 s |
| P9 live HTTPS / P10 official MCP read-only canary | 1/1 / 1/1 PASS | P10 completed in 6.83 s; both remain opt-in |
| M4 `linux_bubblewrap` / `linux_hardened_v1` disposable Linux canaries | 1/1 / 1/1 PASS | Hardened Linux/aarch64 and native x86_64 runs pass; they remain the software-profile evidence beneath the accepted production service. |
| M4 manual native x86_64 acceptance workflow | READY / NOT RUN | The `workflow_dispatch` path remains unrun, but the same checked-in harness produced and locally revalidated a retained native x86_64 bundle on homolab. |
| M4 acceptance-kit validator | PASS / 23/23 | Strict contract/report validation, fail-closed preflight/finalize, exact final config, identity/source/cgroup binding, and atomic no-overwrite success reports |
| M4 lumo live read-only preflight | FAIL CLOSED | Linux `6.18.35-0-rpi` / aarch64 and cgroup v2 are live, but the host exposes only `cpuset cpu io pids`; the required `memory` controller is absent, so lumo is not currently an eligible `linux_hardened_v1` production target |
| M4 homolab disposable native x86_64 canary | PASS / 1/1 | Linux `7.0.10` / native x86_64, fixed Rust image, real bubblewrap/seccomp, cgroup-v2 CPU/memory/pids lifecycle, bounded wrapper, and contract/report validation passed before production selection. |
| Latest signed arm64 APK | PASS | `1.0.3+4`; 52,300,150 bytes; SHA-256 `b9cede23fc918c2d1a76c3ce3ef5f72a3a1680716ef0c6b6b58c038997f56079`; v1/v2, retained certificate `503f8658…b58ee1`, no bundled model/audio |
| Prior signed APK: exact model install + force-stop/airplane cold start | PASS | Model remained installed and verified offline; this is not an inference-quality pass |
| Prior signed APK: consented real-speaker review path | FUNCTION PASS / QUALITY FAIL | Editable local draft opened, destination stayed unselected, Send stayed disabled, and no message was sent; exact reference was not retained, so no CER is fabricated |
| Final diagnostics APK on physical Android | HISTORICAL INSTALL/REVIEW PASS | Installed `1.0.2+3` matched the host SHA-256 byte for byte, retained pairing, cold-launched, and one consented exact-reference recording reached editable review with healthy aggregate diagnostics and no send; the later `1.0.3+4` rows supersede this partial checkpoint. |
| Self-hosted-option APK on physical Android | SUCCESS/FAILURE PASS | `1.0.3+4` installed in place with an exact host/device hash match and retained pairing. The healthy route returned an exact-reference editable review and cancelled with no message. With the real homolab unit stopped and health connection refused, another explicitly selected home capture created no local/remote review, fallback, or message; the selected session remained at 0 messages. The service was restored to healthy and the app reset to local. |
| Android streaming baseline vs offline-Paraformer candidate A/B | PASS | Same physical installation/build/APK/corpus verified 50/50 + 3/3 long runs for both engines. Streaming/candidate converted CER: 0.08709/0.06540; code-switch: 0.18855/0.12112; peak RSS: 712,772/864,112 KiB; max RTF: 0.11879/0.08577; no thermal escalation. Retained [JSON](docs/evidence/2026-07-19-p6-6-android-asr-ab.json) |
| Android non-recording model/permission lifecycle | PASS | Signed release permission denial showed no active capture and was restored; delete/reinstall, force-stop interrupted download, cold-start staging cleanup, verified-model persistence, backup exclusion, and zero external residue pass. Isolated current `.uitest` code detected an injected unexpected file as corrupt, recovered to the three exact model hashes, and deleted its test model without touching production |
| Android recording lifecycle and exact sends | PASS | Backgrounding stopped and released AudioRecord with no review/send; force-stop plus cold launch produced no replay or external residue. One reviewed local draft entered the existing session and another entered a distinct new session; PostgreSQL contained each exact user content once. No destination was inferred and no auto-send occurred. |
| Consented production-local real-speaker corpus | PASS / 10/10 | Mean CER `0.132922` ≤ `0.20`, p90 `0.241379` ≤ `0.35`, code-switch mean `0.281980` ≤ `0.30`; zero empty, truncated, or signal-warning items. Audio was neither retained nor uploaded; raw references, hypotheses, capture ids, and device fingerprint were removed after independently recomputed aggregate verification. Retained [JSON](docs/evidence/2026-07-19-p6-6-real-speaker-eval.json). |
| M4 homolab production service | PASS | Persistent NixOS generation, UID/GID `23017`, loopback-only server, exact `cpu memory pids` delegation, clean cgroup ownership, 256 MiB/16-child/750 ms workload peaks, 4× memory headroom, native retained report, and post-switch restart all pass. A microVM is not claimed under the selected trusted-host-kernel contract. |
| P2 bounded dialectic + frozen voice calibration | PASS | Deterministic cadence/off/cap/replay tests and eight positive/negative rubric fixtures pass |
| P2 actual-output General/grounding/Serious rubric | PASS / 3/3 | Retained non-echo `gpt-5.5` public-API [JSON](docs/evidence/2026-07-18-p2-voice-live.json) passes all criteria and exact finals differ from prompts |

### Immediate next task queue

No committed roadmap milestone remains open. Future work is demand-triggered and must preserve the
acceptance boundaries below; any further real-speaker capture remains opt-in and exact-reference.

### Deferred SDK namespace placement

These are roadmap-owned deferred tasks, not loose TODOs:

| Namespace / surface | Target milestone | Placement note |
|---|---|---|
| `memory.*` | **P2/P4 shipped; P8 expansion** | P2 exposes minimum profile/user recall and state capture. P4 owns transactional dream enqueue, supervised fenced workers, durable proposal effects, summaries, Postgres FTS, and skill/memory proposals. P8 adds self-hosted pgvector hybrid recall, richer scoped stores, evidence-backed extraction, correction, and recall-quality gates. |
| `agents.*` | **P3 ✓ / P3+ ✓** | P3 MVP shipped: `agents.run/spawn/parallel/msg`, actor lifecycle, mailbox/roster, `agent://`+`history://` resources, SSE lifecycle events, Handoff mode. P3+ shipped bounded live inbox delivery plus `send`, `broadcast`, `wait`, `inbox`, `list`, `cancel`, and `pipeline` with digest-reference stage wiring, child approval routing, plain-prose enforcement, live-wait DAG acyclicity checks, active restart supervision, sibling-subtree cancellation policy, wall-clock budgets, status lifecycle events, typed parent `SessionEvent` actor/artifact/history links, and actor smoke coverage. |
| `skills.*` / `skill://` reads | **P4/P7.1 shipped; P10 selected import shipped** | P4 emits approval-gated skill proposals from dreaming. P7.1 ships immutable managed versions, atomic activation/rollback, trigger-aware reload, provenance/audit, and capability-gated `skill://` read/list/preview. Selected MCP objects import only through the trusted P10 catalog and closed P9 boundary. |
| `drive.*` | **P5 production-complete** | `tm-drive`, virtual dirs/search, transducers, approval-gated writes, `drive://`, project scopes, research fan-out, durable Postgres organizer/link/tombstone state, CAS, startup hydration, and final restart/client verification pass. |
| `http.*` hardening | **P9 shipped** | P5 research stays local-first over `drive://` docs. P9 supplies exact allowlists, DNS/IP/redirect policy, durable byte/request/time caps, audit, revocation, mutation replay, and secret integration for P10 live research. |
| `secrets.use` | **P9 shipped** | P9 owns opaque egress-scoped handles: the host does not intentionally publish values into the tm heap, artifacts, logs, events, or model context, and exact literal response reflection is redacted. Transformed reflection is not an information-flow guarantee, so authenticated destinations are owner-trusted. |
| `code.ast` / `code.lsp` | **Demand-triggered** | Start only when native patch-based dogfooding demonstrates a concrete structured-edit or navigation gap. It is not on the committed companion critical path. |

### Sequencing constraints

- P6.5 signed physical closeout is complete. P6.6 and full P6 are also closed after the later signed
  lifecycle/failure/current/new matrix and consented production-local real-speaker quality pass; the
  failed first quality attempt remains retained historical evidence.
- P8 provenance, correction, and recall-quality evidence precede Auto-mode persona proposals.
- P9 egress and opaque-secret gates precede every P10 MCP/live-research production path; both
  milestones now pass that order and remain separate boundaries.
- Cloud sync/CRDT, `code.ast`/`code.lsp`, `tm-trace`, and any future second sandbox backend require a
  concrete user or second consumer before entering the execution table.

### Do-not-start-yet list

- Preserve P6.6 physical acceptance as a signed-device claim backed by the retained matrix. Never
  collect new real speech without explicit speaker consent, and never infer CER when the exact
  spoken reference is absent.
- Auto mode may propose persona addenda but may never approve or activate them.
- Aggressive autonomous identity/config/source/deployment rewrites, raw `SOUL.md` mutation, raw shell,
  ambient authority, Firebase/FCM, or additional chat-native tools.

## Crate plan

Current crates/apps: `tm-core`, `tm-llm`, `tm-lang`, `tm-artifacts`, `tm-host`, `tm-modes`,
`tm-agents`, `tm-memory`, `tm-drive`, `tm-egress`, `tm-mcp`, `tm-server`, `apps/tm-cli`, and client scaffolds under `clients/`. The P0a bridge
currently lives in `tm-server::omp_acp`; P2 minimal memory still lives in `tm-server::memory` while
P4 dream queue ownership lives in `tm-memory`; `fs.*` /
`code.*` / `proc.*` live in `tm-host`; actor lifecycle, mailbox, orchestration, and resources live
in `tm-agents`; local-first drive storage, transducers, organizer proposals, links, and search feeds
live in `tm-drive`; exact external transport and secret handles live in `tm-egress`; selected MCP
catalog, Streamable HTTP, schemas, and untrusted envelopes live in `tm-mcp`.

No additional critical-path crate is planned. `tm-trace` is no longer assumed: extract it only when audit/replay has a second
concrete consumer. Existing core crates (§10.1) keep their contracts; future work should extract
product storage/orchestration only when the second concrete user exists.

## Deferred choices outside the audit

These later product/packaging choices do not reopen closed product milestones. M4 is accepted on
homolab for a hostile workload on its trusted owner-controlled host kernel. A future hostile-kernel
or microVM assurance target is a separate, optional deployment contract and is not claimed here:

- **Deployment mode** — foreground CLI, launchd service, Docker, or lumo service packaging.
- **Drive sync** — local-first default (offline-first); optional CRDT-based cloud sync later (P5, §24.5).

See also §15 and §29.
