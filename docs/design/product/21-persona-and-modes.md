# 21. Persona, modes & routing

Canonical: **`SOUL.md`** (identity + router + boundaries), the runtime **`modes.json`** catalog,
and the curated **`skills/`** bundle from the Hermes deployment. This doc translates them into the
runtime's persona layer. **SOUL.md is authoritative for identity and boundaries; `modes.json` is
authoritative for which modes exist at runtime, which skills they declare, and which skills layer
onto any mode.** Runtime mode ids are strings loaded from the catalog, not Rust enum variants.

**Modes and skills are two different things, deliberately kept apart:**

- A **mode** is a *capability envelope* — the small set of things the runtime will let the model do
  right now (`fs.*`, `code.*`, `proc.*`, `agents.*`, or none of the above). There are exactly three:
  General, Serious Engineer, Handoff (§21.2). Switching mode changes what's technically possible.
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

The structure that lets the first dogfood slice be a serious coding agent without becoming a sterile
coding product:

- **Identity** (SOUL.md) — *who* she is. Always present, never overridden.
- **Voice** (miku-voice) — *how* she talks. Intensity **floats by context**:
  `high` (light / stuck / emotional) → `medium` (planning) → `off` (serious / money /
  legal / irreversible / external). The API uses the English values. Chinese voice labels and
  examples belong inside the mode/voice skills, not in the client UI. Rule: **the more serious, the
  fewer 喵.** Cuteness yields to precision, by design.
- **Mode profile** (`modes.json` → `modes[]`) — *what capabilities are available now*: declared
  runtime capabilities, the mode's own declared skills, grouping class, default scope, and voice cap.
  Switchable, and **sticky**: once switched, a mode stays active until the user changes it or
  confirms an `await modes.suggest(...)` proposal — never a silent per-turn revert.
- **Layered skills** (`modes.json` → top-level `skills[]`, bodies at `skills/*/SKILL.md`) —
  *procedural payloads* composed on top of whichever mode is active: always-on, or triggered by
  message content. Independent of mode capability grants.

```
system prompt = [ SOUL identity (constant) ]
              + [ always-on layered skill markdown ]
              + [ active mode's own declared skill markdown ]
              + [ triggered layered skill markdown, if this message matched ]
              + [ voice overlay @ context-appropriate intensity, via miku-voice when loaded ]
              + [ memory: recall context + summary (§22) ]
```

All skill markdown is frontmatter-stripped before composition. Mode id, capability list, voice cap,
and scope are dispatch data the router and host act on — they are never rendered into the prompt as
labels. Miku doesn't read "Mode id: serious_engineer" or "Voice cap: off"; she reads SOUL.md plus
whichever skill markdown resolved for that turn. Voice intensity likewise isn't told to her as a
value — it's implicit in whether `miku-voice` is among the composed skills, and the skill's own
concentration table reads intensity from context.

Rust vendors the default `SOUL.md`, `modes.json`, and bundled skills under
`crates/tm-modes/assets/`. A configured mode/skill asset path may override those files; missing or
unreadable configured assets degrade with warnings but fall back to the bundled defaults so the
default Miku identity and mode catalog stay available.

Catalog skill references are validated against loaded skill assets, for both a mode's `activeSkills`
and the top-level `skills[]` array. A missing reference degrades with a stable warning naming which
side it came from, and composition falls back to a short, generic "default to careful,
capability-appropriate behavior" note in place of that skill's markdown.

In P2, skill loading and mode/skill ids are server-side dispatch concerns only; they are not a
registered `resources.read/list/preview` scheme, and the `skills` global remains `undefined` until
P4/P7 ships the skill proposal/import/reload lifecycle with explicit grants (§7.4 / §9.3).

A mode profile grants capabilities and selects its own skills; it **never** replaces identity. Tone
still comes from the voice overlay, capped by seriousness, so serious work can turn cuteness down
without changing who Miku is. V1 does **not** require Rust to know a fixed list of modes or layered
skills: `modes.json` defines both, and adding/removing either should not require a Rust enum change.

## 21.2 Capability modes (SOUL.md §Mode Router)

Pick the smallest sufficient mode — and expect it to stick until you leave it.

| # | Bundled runtime mode | Declared capabilities | Voice | Mode-declared skills |
|---|---|---|---|---|
| 1 | **General** (default) | conversation + light `memory.recall` / `memory.propose` | `medium` | `miku-voice`, `personal-assistant-state-capture` |
| 2 | **Serious Engineer** | native `fs.*` / `code.*` / `proc.*` and `resources.read:linked` (§25), with P0a OMP ACP still available as a replaceable bridge; future light `agents.*` | `off` | `serious-engineer-ops` |
| 3 | **Handoff** | `agents.*` (§23) + brief generation | `off` | `oh-my-pi-handoff` |

All three modes run under the **one** character. Capabilities are config, not code (§10.4): a mode =
runtime id + profile + route triggers (routing hints; see §21.4) + declared capabilities + its own
declared skills + default scope + voice cap. The optional `capabilityClass` is only grouping/display
metadata; dispatch decisions use declared capabilities such as `backend.coding`, `fs.*`, `code.*`,
`proc.*`, or `agents.*`.

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
them from layering onto Serious Engineer or Handoff too if the trigger words show up there. This is
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

- **Capability modes are sticky.** Once a session is in Serious Engineer or Handoff, it stays there
  across turns regardless of what later messages say — there is no per-turn keyword revert. Getting
  back to General is a deliberate action: the user's mode picker, an explicit `override`/`apply`
  call, or a confirmed `await modes.suggest(...)` (below). A locked mode suppresses all of these
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
- **v1 scoping: `modes.suggest` authority only exists on unlocked normal ChatRunner turns.** The
  server registers the host handler independently, then adds the exact `modes.suggest` grant only to
  unlocked normal chat turns. Native coding backends (§25), actors, scheduler runs, and locked turns
  never receive that grant. So once a
  turn is actually dispatched to a configured coding backend (i.e. Serious Engineer or Handoff with
  a real backend wired in), the model has no way to propose *leaving* that mode from inside it —
  getting back to General is picker/override-only in that state. Proposing to *enter* Serious
  Engineer or Handoff from General works today because General always runs the ChatRunner path.
  Extending mode-suggestion authority to the coding backend is explicitly out of the v1 contract.
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
    SDK-->>C: ModeChanged(Serious Engineer)  %% debug/advanced only; voice cap is internal
    SDK-->>M: execute result: approved; next turn gets engineering grants
    Note over M: identity unchanged; precise, asks before destructive
    Note over M,SDK: A later "clean up my inbox" does NOT revert the mode — it stays in<br/>Serious Engineer until Brian or another confirmed modes.suggest moves it.
```

## 21.5 Character: defined (was the top open question)

**Resolved.** The character is **Tempest Miku** per `SOUL.md` + `miku-voice`. P0/P1 (§28) ship the
real persona in Serious Engineer and project-manager form first; P2 broadens into the full
General baseline without replacing the identity. Voice mechanics (喵 density, 主人 honorific,
self-reference, catchphrases, example lines) live in `miku-voice` and load as a **voice overlay at
context-appropriate intensity** — never baked into a mode addendum, so seriousness can dial it down.
