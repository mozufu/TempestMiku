# One-shot plan: P7.0 self-evolution safety foundation

Approved: **2026-07-13**.

`ROADMAP.md` remains canonical for milestone order. `TODO.md` is the executable checklist for this
slice.

## Alignment Brief

- **Outcome:** establish enforceable self-evolution tiers, durable audit/provenance, and replayable
  review flows before enabling any live skill, persona, mode, prompt, config, or source mutation.
- **Audience:** the single-owner TempestMiku deployment and contributors implementing the next
  roadmap slice.
- **Success criteria:** tier changes alter reachable write targets at the registry/effect boundary;
  conservative authority is limited to existing memory/profile and skill-proposal paths; moderate
  produces reviewable approval-backed diffs without applying them; aggressive remains unavailable;
  replay explains every attempt and outcome.
- **Must-haves:** fail-closed config and dispatch, exact target classes, durable audit records,
  redaction and bounded payloads, approval/default-deny behavior, restart/idempotency tests, public
  event/resource evidence, and docs/SDK/TODO/ROADMAP alignment.
- **Non-goals:** live catalog install/reload, applying persona/mode diffs, aggressive execution,
  MCP, trace extraction without a second user, live egress, secret broker, cloud sync, richer memory
  retrieval, and blocked production push work.
- **Constraints:** preserve one model-visible `execute(code)` tool, streaming-first events, current
  approval/resource boundaries, hand-owned `SOUL.md`, no raw shell or ambient access, and
  external-service-free normal tests.
- **Key decisions:** self-evolution remains a policy layer across existing crates; tiers are upper
  bounds rather than grants; target classes are typed and never arbitrary paths; authority is
  rechecked immediately before effects; P4 `SkillProposalRecord` is reused; P7.0 review does not
  mutate the live catalog or persona/mode files.
- **Implementation approach:** freeze policy/wire contracts, add config plus least-authority
  enforcement, persist transactional audit state, harden the conservative skill-proposal boundary,
  expose the moderate review lifecycle without apply, and close with deterministic plus gated
  replay/restart evidence.
- **Acceptance checks:** use the P7.0 acceptance gate and section-level checks in `TODO.md`; finish
  with strict Rust gates, gated Postgres coverage, focused Flutter/Web smoke, and
  `tm-e2e record evolution-policy` evidence.
- **Open question:** the eventual production Android provider remains blocked and intentionally
  independent of P7.0. Provider selection requires a separate explicit decision.

## Sources To Read Before Implementation

- `AGENTS.md`
- `ROADMAP.md`, especially the execution queue and P7 row
- `TODO.md`
- `docs/design/product/26-self-evolution.md`
- `docs/design/product/22-memory-dreaming.md`
- `docs/design/product/27-server-and-clients.md`
- `docs/design/core/07-host-sdk.md`
- `docs/design/core/08-security-model.md`
- `docs/design/core/09-context-artifacts.md`

## Fresh-thread Prompt

Use this plan as the project brief. First read the whole brief and the listed source files, then
implement the smallest unchecked P7.0 section from `TODO.md`. Preserve the stated constraints,
non-goals, and acceptance checks. Keep code, tests, checked-in SDK types, design docs, `ROADMAP.md`,
and `TODO.md` aligned in the same patch. Do not enable live skill installation, persona/mode apply,
aggressive self-evolution, MCP, live egress, or production push. Prove behavior with focused tests
and replayable public-surface evidence before marking any acceptance item complete. If anything is
ambiguous, ask only the smallest blocking question before building.
