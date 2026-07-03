# 29. Parity baseline — the current deployment

The Rust TempestMiku is a **rewrite of a running system**. This doc pins what exists today so the
rewrite can reach **behavioral parity before adding anything new**.

## 29.1 Where it lives

`~/deployment-config/hosts/lumo/home/services/hermes-agent/` (host `lumo`). Service name `hermes-agent`;
Honcho host id `hermes`; Honcho workspace `tempest-miku`, peer `brian`.

## 29.2 Canonical behavioral spec (preserve)

| File | Role | Maps to |
|---|---|---|
| `SOUL.md` | identity, modes, boundaries | §21 |
| `skills/miku-voice` | voice overlay (喵 density, honorifics, lines) | §21.1 |
| `skills/ambiguity-grill` | mode 2 | §21.2 |
| `skills/negative-state-grounding` | mode 3 | §21.2 |
| `skills/oh-my-pi-handoff` | mode 5 + handoff brief template | §21.2, §23 |
| `skills/personal-assistant-state-capture` | memory discipline | §22.8 |
| `skills/scope-guard` | pushback / anti-new-pit | §21.3 |
| `skills/weekly-ship-ledger` | weekly review (cron) | §27.2 |
| `honcho.json` | memory **behavior** target — recall/ToM/budgets, built natively | §22.7 |
| `config.yaml` | models, capabilities, approvals | §29.3 |

## 29.3 Current runtime facts (`config.yaml`)

- **Models:** provider `custom`, default `gpt-5.5`, fallback `gpt-5.4-mini`; `chat_completions` API.
- **Aliases:** `daily` / `heavy` / `cheap` / `openai-heavy` / `coding-plan` / `code-review`.
- **Auxiliary roles:** `compression`, `web_extract`, `title_generation`, `approval`, `skills_hub`,
  `mcp`, `triage_specifier`, `kanban_decomposer`, `profile_describer`, `curator`.
- **Capabilities present:** terminal (local, cwd `/opt/workspace`, 180s) → Rust uses `proc.run`
  (§25.2); skills hub (`write_approval`); MCP (`mcp_reload_confirm`); cron (`cron_mode: deny`);
  memory (honcho, `memory_enabled`, `user_profile_enabled`, `write_approval`).
- **Approvals:** `mode: manual`, 60s timeout.
- **Budgets:** `goals.max_turns: 8`; tool output 50000 B / 2000 lines / 2000 line-length (§25.3).

## 29.4 What the rewrite changes on purpose

- **Single `execute` tool + SDK** (the bet) replaces a multi-tool surface (§01, §25.1).
- **Raw terminal → curated `proc.run`** (allowlisted, approval-gated, linked-folder) (§25.2).
- **Adds:** persistent character chat over a streaming server + WebUI / Android (§27);
  code-as-orchestration `agents.*` (§23); drive auto-organize (§24); self-evolution tiers (§26).
- **Replaces Honcho** (today's memory service) with a self-built `tm-memory` engine — episodic +
  vector + BM25 + facts fused by RRF + memory-stream scoring; the user model / ToM and "dreaming"
  are ours, no external memory dependency (§22).
- **Adds the P2 memory resource gateway:** `memory://root`, `memory://user-model`, and approved
  profile fact / scoped recall record URIs resolve through the same preview/list/read path as artifacts
  and linked resources, with unknown or ungranted memory paths denied (§22.9 / §27.5).
- **Adds P2.5 state capture:** `skills/personal-assistant-state-capture` is enforced server-side as
  approval-backed memory proposal logic for stable preferences, personal reminders, open loops,
  commitments/deadlines, decisions, shipped artifacts, and workflows; transient moods, secrets, raw
  logs, one-off complaints, large notes, and obvious sensitive PII are skipped before proposal creation
  (§22.8 / §27.6).
- **Keeps:** SOUL identity + 5 modes (§21); the **memory behavior** — hybrid recall, user profile +
  ToM, async write, ~1600-tok context, write-approval (§22, now self-built); the 7 skills (§26.2);
  manual approvals + proactivity bounds (§21.3); cron-driven reviews (§27.2); model-role system (§27.3).

## 29.5 Parity gate

P0–P4 (§28) are not "done" until they reproduce the current behavior for their slice: coding reach
works through the single `execute` SDK (§25), project continuity survives across sessions (§22/§27),
Miku replies in-voice (§21), the mode router fires (§21.2), memory recall + user profile work (§22),
and approvals gate the same actions (§21.3). New capabilities layer on **after** parity, not before.
