# TempestMiku Future Roadmap Alignment Brief

Approved: **2026-07-14**. Sequencing amended: **2026-07-15**.

## Outcome

Finish TempestMiku as a self-hosted daily companion with strong Android entry points, durable
project-management and coding modes, trustworthy long-term memory, and a deliberately ordered path
to external tools. The product remains one characterful companion rather than a generic agent
marketplace.

## Audience And Primary Workflows

- The owner uses Miku throughout the day from Android and Web/PWA.
- Personal-assistant workflows are the product north star.
- Serious Engineer and project-manager modes remain first-class: they inspect real repositories,
  tests, evidence, plans, and open loops, then discuss progress or execute approved work through the
  existing bounded coding runtime.
- PM value is conversational and evidence-backed; a separate management dashboard is not a roadmap
  prerequisite.

## Success Criteria

- Android supports notification reply and fast capture without creating a second agent loop or
  ambient authority. Voice capture remains a bounded P6.6 design, explicitly deferred after its
  feasibility spike.
- P6.1-P6.5 remain closed; P6.6 and full P6 closeout remain incomplete and unscheduled until the
  owner explicitly resumes them.
- Fuller memory improves measured recall quality using self-hosted storage and embeddings.
- Auto mode may decide when a persona improvement is worth proposing, but a human always approves
  activation; every version is immutable and rollbackable.
- Production egress and secrets are hardened before MCP or live external research can ship.
- Every stage has deterministic gates plus physical or live evidence proportional to its risk.

## Product And Architecture Direction

- Optimize first for daily companion value, then for coding/PM depth.
- Self-host whenever practical. External providers may exist only behind replaceable, explicitly
  configured interfaces with a safe disabled/degraded mode.
- Preserve one model-visible tool: `execute(code)`; MCP capabilities translate into SDK/resource
  namespaces rather than chat-native tool sprawl.
- Preserve streaming-first execution, durable turns, replayable SSE, exact grants, manual approvals,
  server-owned authority, and fail-closed unknown values.
- OMP ACP remains replaceable. Native Deno remains the coding dogfood path.

## Approved Execution Order

The owner amended the order on 2026-07-15: P6.6 is deferred with retained evidence, and P8 is the
active next milestone. This removes the former P6.6-before-P8 sequencing constraint without marking
P6.6 or P6 complete. The active route is now P8 → P7.2b → P9 → P10; P6.6 has no scheduled position.

### P6.4 — Actionable Notifications

- Deep-link each notification to its exact session or approval context.
- Accept bounded inline-reply text only after the owner explicitly presses Send.
- Post through the existing authenticated durable message API; native code does not run a model,
  sandbox, or second message loop.
- Preserve device authentication, generic lock-screen content, notification cancellation, leased
  delivery, retry bounds, and exact-once client-message semantics.
- Prove foreground, background, killed-process, retry, duplicate-intent, and cold-start behavior on
  a signed Android 15 upgrade.

### P6.5 — Quick Capture

- Define one bounded capture intent and editable composer flow.
- Ship a launcher shortcut first, then a Quick Settings tile; add a small widget only if it reuses
  the same intent and adds no background send path.
- Entry points may prefill content or destination hints, but they never send, pair, approve, or grant
  authority before the owner reviews and confirms.
- Prove cancel/no-send and warm/cold-start exact-once behavior.

### Deferred — P6.6 On-Device Voice Capture And P6 Closeout

- Record at most 60 seconds of PCM16-LE, 16 kHz, mono audio on Android and keep raw audio on-device
  and ephemeral.
- Put inference behind a replaceable `LocalAsrEngine`; do not add a server transcription route or
  automatic platform/cloud/lumo fallback. Missing or unsupported local models fail visibly.
- Use SenseVoice Small INT8 through sherpa-onnx only as the first benchmark baseline. No production
  model is selected; NVIDIA Parakeet zh-TW also remains a candidate only after portable weights,
  redistribution, conversion, and the same physical-device gates pass.
- Download a versioned model only after explicit owner action, verify SHA-256, cap it at 350 MiB,
  store it app-private outside backup, and expose delete/reinstall controls.
- Show an editable transcript and explicit current/new-session destination before sending.
- Bound inference to 45 seconds and define cancellation, explicit retry, process-death cleanup, and
  no-replay behavior.
- Close P6 after deterministic client/runtime, pinned-model RTF/RSS/thermal, airplane-mode,
  signed-build, and physical Android evidence passes.

The retained interface/harness work and all 2026-07-15 evidence are centralized in
[`docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md`](../docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md).
That note records the raw report hashes, model/corpus provenance, Android metrics, Taiwan-Mandarin
quality gaps, wireless ADB proof, open acceptance gates, prohibited work while deferred, and the
explicit resume contract. It is not milestone acceptance.

### Active — P8 Fuller Memory

- Promote the self-hosted Postgres spine to hybrid Postgres FTS plus pgvector recall.
- Use a local embedding provider by default; keep dimensions/version provenance and deterministic
  re-embedding behavior explicit.
- Add scoped episodic and semantic stores, summaries, profile facts, provenance, confidence,
  correction/supersession history, fusion, reranking, and context budgeting.
- LLM-backed extraction produces candidates with evidence; it never silently converts inference
  into owner truth.
- Measure recall relevance, scope isolation, stale-fact handling, degradation, and replay rather
  than accepting successful writes as proof of memory quality.

### P7.2b — Approval-Backed Persona Addenda

- Auto mode may detect repeated preferences or persona mismatch and create a typed proposal.
- Proposals require evidence, deduplication, cooldown, bounded previews, and stable base digests.
- Every activation and rollback requires durable manual approval.
- Store immutable persona-addendum versions and compose the approved version on the next turn.
- Permit bounded tone, address, and interaction-preference guidance only.
- Never mutate `SOUL.md`, core Miku identity, safety rules, capabilities, voice caps, mode scopes,
  route triggers, source code, configuration, or deployment.

### P9 — Production Egress And Secret Broker

- Add destination allowlists, DNS/IP and redirect policy, request/byte/time budgets, audit events,
  revocation, and explicit per-turn grants.
- Resolve opaque, egress-scoped secret handles only at the host boundary. Secret values never enter
  the JS heap, prompt, artifacts, event payloads, or logs.
- Keep production egress disabled until denial, timeout, redirect, exfiltration, replay, and secret
  redaction gates pass.

### P10 — MCP And Live Research

- Import selected MCP resources/prompts/tools through a configured catalog.
- Translate them into lazy SDK/resource surfaces behind P9 authority; do not add model-visible tools
  beside `execute(code)`.
- Require provenance, bounded schemas/results, approval for mutation, catalog reload controls,
  prompt-injection isolation, and complete audit/replay coverage.
- Build live research on the same egress boundary instead of adding a parallel HTTP escape hatch.

## Demand-Triggered Work

- Cloud drive sync and CRDT/conflict UX: start only when a real multi-store synchronization workflow
  exists; local-first remains authoritative.
- `code.ast` / `code.lsp`: start only when patch-based dogfooding shows a concrete structured-edit or
  navigation gap.
- `tm-trace`: extract only after audit/replay has a second concrete consumer.
- Additional sandbox backends: add only for a workload the Deno backend cannot reasonably serve.

## Permanent Non-Goals

- Aggressive autonomous identity, configuration, source-code, or deployment rewrites.
- Automatic persona approval or activation.
- Direct mutation of `SOUL.md` or hand-authored modes/skills.
- Raw shell, ambient filesystem access, ambient network access, or secret material in model context.
- Firebase/FCM; UnifiedPush/ntfy remains the production push path.
- A generic agent marketplace or hundreds of chat-native tools.

## Acceptance And Closeout Rules

- Each slice updates code, focused tests, full affected gates, SDK/types, design docs, `ROADMAP.md`,
  and `TODO.md` together.
- Android slices require merged-manifest/APK inspection, established-certificate verification,
  in-place upgrade, and physical Android 15 evidence.
- Server authority changes require denial, timeout/default-deny, stale/replay, idempotency, restart,
  and gated Postgres coverage.
- Self-hosted services require loopback/public health, restart persistence, failure/degradation, and
  secret/configuration evidence.
- A milestone is not complete from prose alone; preserve replayable `tm-e2e` or equivalent evidence.

## Rollback And Risk Boundaries

- P6 additions reuse existing authenticated APIs and can be disabled without changing core turns.
- Memory mechanisms degrade to lexical/profile/summaries when embeddings are unavailable.
- Persona addenda use immutable versions and an atomic active pointer; rollback restores a previous
  version or the hand-authored base.
- Egress and MCP remain disabled by default and fail closed when configuration is incomplete.

## Source Files

- `ROADMAP.md` — canonical milestone order and completion history.
- `TODO.md` — executable checklist for the active slice.
- `AGENTS.md` — agent sequencing and invariants.
- `docs/design/product/22-memory-dreaming.md` — fuller-memory architecture.
- `docs/design/product/26-self-evolution.md` — persona proposal authority.
- `docs/design/product/27-server-and-clients.md` — Android/server integration contracts.
- `docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md` — retained P6.6 evidence and resume
  contract.

## Fresh-Thread Prompt

Use this plan as the project brief. First read the whole brief, then inspect `ROADMAP.md`, `TODO.md`,
the relevant design sections, and the current worktree. Implement only the active milestone in the
approved order. Preserve the stated companion-first direction, self-hosting preference, authority
boundaries, non-goals, and acceptance checks. If anything is ambiguous, ask only the smallest
blocking question before building. Keep implementation, tests, SDK/docs, roadmap status, and public
evidence aligned in the same change.
