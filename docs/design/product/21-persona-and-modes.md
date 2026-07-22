# 21. Persona, modes & routing

Canonical: **`SOUL.md`** (identity + router + boundaries), the runtime **`modes.json`** catalog,
and the curated **`skills/`** bundle from the Hermes deployment. This doc translates them into the
runtime's persona layer. **SOUL.md is authoritative for identity and boundaries; `modes.json` is
authoritative for which modes exist at runtime, which skills they declare, and which skills layer
onto any mode.** Runtime mode ids are strings loaded from the catalog, not Rust enum variants.

**Modes and skills are two different things, deliberately kept apart:**

- A **mode** is a *capability envelope* — the small set of things the runtime will let the model do
  right now (`fs.*`, `proc.*`, the exact 15-call curated `git.*` namespace in §25.2.2, `agents.*`,
  or none of the above). There are exactly two: General and Serious Engineer (§21.2). Switching mode
  changes what's technically possible.
- A **skill** is a *procedural payload* — prompt markdown that shapes how Miku behaves, layered onto
  whichever mode is currently active. Skills never change what's possible, only how it's approached.
  Some skills are declared by a specific mode (`serious-engineer-ops` only makes sense inside Serious
  Engineer); some are always-on regardless of mode (`scope-guard`); some are keyword-triggered
  regardless of mode (`ambiguity-grill`, `negative-state-grounding`, `weekly-ship-ledger`) (§21.3).

Earlier revisions of this doc modeled Ambiguity Grill and Negative-State Grounding as their own
*modes* even though they granted zero capabilities — every mode-switch they caused was purely
cosmetic, and the keyword router that drove all mode switching silently reverted to the default on
the very next unrelated message. That conflation is why the split above exists: conversational
posture and capability grants are orthogonal, and only one of them should require a mode switch.

## 21.1 Three layers: identity constant, voice floats, capability modes, layered skills

The structure that lets serious coding coexist with a characterful companion instead of becoming a
sterile coding product:

- **Identity** (SOUL.md) — *who* she is. Always present, never overridden.
- **Voice agreement** (`SOUL.md` + mode/voice skill text) — *how* she talks. Intensity floats by
  context: light / stuck / emotional is more characterful; planning is moderate; serious / money /
  legal / irreversible / external work is plain and precise. This is prompt guidance shared by the
  identity and the active mode's skills, not a structured profile field, API value, router input, or
  client setting. Rule: **the more serious, the fewer 喵.** Cuteness yields to precision, by design.
- **Mode profile** (`modes.json` → `modes[]`) — *what capabilities are available now*: declared
  runtime capabilities, the mode's own declared skills, grouping class, and default scope.
  Switchable, and **sticky**: once switched, a mode stays active until the user changes it or
  confirms an `await modes.suggest(...)` proposal — never a silent per-turn revert.
- **Layered skills** (`modes.json` → top-level `skills[]`, bodies at `skills/*/SKILL.md`) —
  *procedural payloads* composed on top of whichever mode is active: always-on, or triggered by
  message content. Independent of mode capability grants.

```
system prompt = [ immutable tm runtime boot contract ]
              + [ runtime-owned tm-lang-fluency skill ]
              + [ SOUL identity (constant) ]
              + [ always-on layered skill markdown ]
              + [ active mode's own declared skill markdown ]
              + [ triggered layered skill markdown, if this message matched ]
              + [ voice behavior from SOUL.md and composed mode/voice skill text ]
              + [ memory: recall context + summary (§22) ]
```

The core agent loop prepends the small immutable boot contract on every path, including custom
persona and actor prompts. The mode composer then inserts the bundled `tm-lang-fluency` skill before
SOUL and mode-selected skills. Configured or managed assets cannot replace or disable this
runtime-owned language guidance. Repo workflow details such as `fs.patch` operations remain in
`serious-engineer-ops`, not in the language contract.

Mode id, capability list, and scope are dispatch data the router and host act on; they are never
rendered into the prompt as labels. Miku doesn't read "Mode id: serious_engineer" or a structured
voice level. She reads SOUL.md plus whichever skill markdown resolved for that turn. Voice behavior
is therefore changed by editing those prompt assets, not by changing API metadata.

Rust vendors the default `SOUL.md`, `modes.json`, and bundled skills under
`crates/tm-modes/assets/`. A configured mode/skill asset path may override those files except the
runtime-owned `tm-lang-fluency` skill; missing or unreadable configured assets degrade with warnings
but fall back to the bundled defaults so the default Miku identity and mode catalog stay available.

Catalog skill references are validated against loaded skill assets, for both a mode's `activeSkills`
and the top-level `skills[]` array. A missing reference degrades with a stable warning naming which
side it came from, and composition falls back to a short, generic "default to careful,
capability-appropriate behavior" note in place of that skill's markdown.

Skill loading and mode/skill ids are server-side dispatch concerns only; they are not a
registered `resources.read/list/preview` scheme, and the `skills` global remains `undefined` until
the explicit managed-skill resource and approval lifecycle (§7.4 / §9.3 / §26.4).

A mode profile grants capabilities and selects its own skills; it **never** replaces identity. Tone
still comes from the voice overlay, capped by seriousness, so serious work can turn cuteness down
without changing who Miku is. Rust does not know a fixed list of modes or layered skills:
`modes.json` defines both, and adding/removing either does not require a Rust enum change.

## 21.2 Capability modes (SOUL.md §Mode Router)

Pick the smallest sufficient mode — and expect it to stick until you leave it.

| # | Bundled runtime mode | Declared capabilities | Mode-declared skills |
|---|---|---|---|
| 1 | **General** (default) | conversation + model-controlled `memory.search`; approval-backed state capture | `miku-voice`, `personal-assistant-state-capture` |
| 2 | **Serious Engineer** | `backend.coding`; native `fs.*` / `code.*` / `proc.*`; the exact `git.clone` / `git.init` / `git.add` / `git.mv` / `git.restore` / `git.rm` / `git.bisect` / `git.status` / `git.diff` / `git.grep` / `git.log` / `git.show` / `git.commit` / `git.push` / `git.pull` grants (§25.2.2); `agents.*`; `resources.read:linked` / `resources.read:agent` / `resources.read:history` (§23, §25). Only status/diff/grep are approval-free; log/show and every Git mutation/network call are always approved. The OMP ACP bridge is a replaceable coding backend. | `serious-engineer-ops` |

Both modes run under the **one** character. Capabilities are config, not code (§10.4): a mode =
runtime id + profile + route triggers (routing hints; see §21.4) + declared capabilities + its own
declared skills + default scope. The optional `capabilityClass` is only grouping/display metadata;
dispatch decisions use declared capabilities such as `backend.coding`, `fs.*`, `proc.*`, each exact
`git.<operation>` grant from the 15-call namespace in §25.2.2, or `agents.*`; there is no wildcard
`git.*` grant, `git.run`, raw argv, or shell-shaped Git capability.
Voice remains prompt-level agreement rather than mode metadata. The catalog contains only these two
mode ids: `handoff` is rejected as unknown, with no compatibility alias.

What used to be modes 2 and 3 here (Ambiguity Grill, Negative-State Grounding) granted no
capabilities and are now layered skills instead — see §21.3. They still shape the conversation the
same way; they just no longer require, or cause, a capability-mode switch to do it.

## 21.3 Layered skills: always-on and triggered, independent of mode

The catalog's top-level `skills[]` array declares layered skills, each with an `activation`:

| Skill | Activation | Purpose |
|---|---|---|
| **scope-guard** | always-on | Pushback on new pits, over-engineering, research-as-procrastination. Catchphrase *"別再開新坑了，你要做不完了。"* Composes into every turn regardless of mode. |
| **ambiguity-grill** | triggered (「燒烤」/ "grill me" / "ambiguous") | Vague / contradictory / hiding-the-real-problem requests get 3–7 sharp questions, then compressed into a plan. **燒霧不燒人.** |
| **negative-state-grounding** | triggered (overwhelmed / exhausted / spiraling / stuck / self-deprecating language, incl. Chinese equivalents) | Stabilize first, then reduce to at most one ≤10-minute action. Health-over-productivity wins even when it blocks the work plan. Purely a conversational posture — does not touch capabilities, and does not itself gate memory writes (see below). |
| **weekly-ship-ledger** | triggered (shipping/productivity self-assessment language) | Count small-but-real shipped things; also reachable via cron (§27). |

Layered skills compose on top of *whatever mode is active* — most often General, but nothing stops
them from layering onto Serious Engineer too if the trigger words show up there. This is
the fix for the two skills that used to be dead code: earlier revisions of `modes.json` bundled
`scope-guard` and `weekly-ship-ledger` as skill assets but never attached them to any mode's
`activeSkills`, so despite this doc describing them as "always on, any mode," the composer had no
path to load them. The top-level `skills[]` array is that path.

Standing behavior from SOUL.md that isn't a skill file but is still cross-cutting:

- **Proactivity: high, bounded.** *Safe, run freely:* organize, research, build TODOs / light
  plans, draft, summarize open loops. *Needs explicit permission:* send message / email, publish,
  spend money, delete / destructive, external commitments, store sensitive PII. Maps to the runtime
  `ApprovalPolicy` (§08, §10.2); current deployment runs `approvals: manual` (§29).
- **Health Override:** body / nervous system **>** productivity. A rule, not a joke.
- **Memory Discipline** (§22.8): remember stable signal, not moods / secrets / raw logs. Because
  distress messages now stay in General (which has `personal-assistant-state-capture` active,
  unlike the old capture-free Negative-State-Grounding mode), the state-capture transient-mood guard
  is the *only* remaining safeguard against turning a bad moment into a memory write, and its word
  list is kept in sync with negative-state-grounding's trigger list for that reason. A pure distress
  message is skipped; a mixed message ("overwhelmed, but remind me to call the landlord") still
  captures the durable part — that's intended, not a leak.

## 21.4 Routing: model-proposed, user-confirmed, sticky, observable

- **Capability modes are sticky.** Once a session is in Serious Engineer, it stays there across turns
  regardless of what later messages say — there is no per-turn keyword revert. Getting back to
  General is a deliberate action: the user's mode picker, an explicit `override`/`apply` call, or a
  confirmed `await modes.suggest(...)` (below). A locked mode suppresses all of these
  except unlock.
- **Switching a mode is model-proposed, user-confirmed — not silently auto-applied.** The model
  calls `await modes.suggest(targetMode, reason)` from inside `execute` when it judges the
  conversation needs a different capability envelope (e.g. real repo work, or delegating to a
  coding agent). `modes.suggest` is a grant-controlled host SDK function, not a second model-visible
  tool. That surfaces as a pending confirmation (the existing approval-broker event/resolve flow,
  §10.2/§25) rather than
  an immediate switch; the model gets the user's decision back as its tool result and continues the
  turn coherently either way. The bundled server's keyword `route.triggers` on each mode remain in
  the catalog as a cheap signal for `modes.suggest` to reason from, but they never auto-apply a switch
  by themselves — that was the old, confusing behavior (a substring match on words like `repo` or
  `test` could silently flip the whole session's capability envelope, then flip back on the very next
  unrelated message).
- **User is always final.** Brian can **lock** a mode, **override** it directly, or confirm/decline
  any `modes.suggest` — a locked mode suppresses proposals entirely rather than prompting for one.
  If the session is locked or moved to another mode while a suggestion is pending, the stale
  suggestion is ignored even if its approval response arrives later.
- **Scoped authority:** `modes.suggest` authority exists only on unlocked normal `ChatRunner` turns.
  The server registers the host handler independently, then grants the exact capability only to that
  path. Native coding backends (§25), actors, scheduler runs, and locked turns never receive it.
  Therefore General can propose entering Serious Engineer; after a turn is dispatched to a coding
  backend, leaving that mode is a picker/override action. Widening that authority remains
  demand-triggered.
- **Observable, not primary UI.** Each switch emits `ModeChanged` (§10.2) for replay, audit, and
  optional debug/advanced controls. Clients read `GET /modes` to render runtime mode controls;
  default chat surfaces should not make Brian manage or care about the mode label. Mode is a
  capability envelope behind the interaction; skills are the texture on top of it.

```mermaid
sequenceDiagram
    participant B as Brian
    participant M as Miku (model)
    participant SDK as modes.suggest host capability
    participant C as Clients
    B->>M: "this migration scares me, can you do it?"
    M->>SDK: execute("await modes.suggest('serious_engineer', 'irreversible op')")
    SDK-->>C: pending confirmation (approval event)
    C-->>SDK: Brian confirms
    SDK-->>C: ModeChanged(Serious Engineer)  %% debug/advanced capability state only
    SDK-->>M: execute result: approved; next turn gets engineering grants
    Note over M: identity unchanged; precise, asks before destructive
    Note over M,SDK: A later "clean up my inbox" does NOT revert the mode — it stays in<br/>Serious Engineer until Brian or another confirmed modes.suggest moves it.
```

## 21.5 Character: defined (was the top open question)

The character is **Tempest Miku** per `SOUL.md` + `miku-voice`. Serious Engineer,
project-manager, and General workflows share that identity. Voice mechanics (喵 density, 主人
honorific, self-reference, catchphrases, example lines) live in `miku-voice` and load as a **voice
overlay at context-appropriate intensity** — never in a mode addendum, so seriousness can dial it down.

The output boundary is verified in two layers. `tm-modes::evaluate_voice` and the frozen
`p2_voice_rubric_v1.json` fixture deterministically calibrate positive and negative General,
negative-state grounding, and Serious examples. Actual model parity is a separate opt-in
`tm-e2e voice-eval` public-API run over exact final responses; passing prompt-composition tests or the
frozen fixture alone does not prove that live model output is in voice. The retained 2026-07-18
non-echo run passes all three scenarios; see
[`live voice evidence`](../../evidence/2026-07-18-p2-voice-live.json).
