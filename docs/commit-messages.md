# Commit message spec

TempestMiku uses a strict Conventional Commit dialect so history stays readable by milestone,
crate, app, and product surface.

## Format

```text
<type>(<machine>/<scope>): <subject>

[optional body]

[optional footers]
```

Required header fields:

- `type`: one of the allowed types below.
- `machine`: the stable repo area changed.
- `scope`: the smallest meaningful component inside that area.
- `subject`: an imperative, lowercase summary of the change.

Examples:

```text
feat(core/agent-loop): stream tool-call deltas
fix(sandbox/tm): default-deny unknown http hosts
docs(repo/commits): define commit message rules
test(server/sse): cover approval denial events
```

## Allowed types

| Type | Use when the commit primarily... |
|---|---|
| `feat` | adds or exposes behavior, capability, API, UI, or product surface |
| `fix` | corrects broken behavior, wrong output, security policy, or regression |
| `docs` | changes documentation only |
| `style` | changes formatting only; no behavior, API, or doc meaning changes |
| `refactor` | restructures code without intended behavior change |
| `perf` | improves performance, allocation behavior, latency, or throughput |
| `test` | adds or changes tests, fixtures, or test helpers only |
| `build` | changes dependencies, build scripts, packaging, lockfiles, or workspace setup |
| `ci` | changes CI, release, or repository automation |
| `chore` | performs repository maintenance that fits no more specific type |
| `revert` | reverts one or more earlier commits |

Pick the type by intent, not by the largest file count. If a behavior fix also adds tests,
use `fix`, not `test`. If a feature needs docs in the same commit, use `feat`, not `docs`.
Split commits when two unrelated intents are present.

## Machine and scope

Use `machine/scope`, not a bare scope. `machine` is a stable top-level product or repo area;
`scope` is the narrow component a maintainer would search for later.

Preferred machines:

| Machine | Typical scopes | Examples |
|---|---|---|
| `core` | `agent-loop`, `messages`, `events`, `llm`, `sandbox-trait` | `core/agent-loop` |
| `llm` | `openai`, `sse`, `errors`, `fixtures` | `llm/sse` |
| `sandbox` | `tm`, `sdk`, `artifacts`, `security` | `sandbox/tm` |
| `host` | `approval`, `fs`, `proc`, `code`, `policy` | `host/approval` |
| `artifacts` | `store`, `resource`, `spill`, `refs` | `artifacts/store` |
| `persona` | `modes`, `router`, `overlay`, `voice` | `persona/modes` |
| `server` | `sse`, `api`, `sessions`, `approvals`, `memory` | `server/sse` |
| `cli` | `args`, `streaming`, `config`, `sandbox` | `cli/streaming` |
| `web` | `approvals`, `chat`, `events`, `styles` | `web/approvals` |
| `docs` | `design`, `roadmap`, `product`, `core`, `commits` | `docs/roadmap` |
| `repo` | `workspace`, `deps`, `nix`, `agents`, `commits` | `repo/workspace` |

Rules:

- Prefer product/architecture names over file names: `core/agent-loop`, not `core/lib`.
- Prefer the narrowest stable scope with a real owner.
- Use `repo/*` for cross-cutting repository mechanics.
- Use `docs/*` only for documentation-only commits.
- For changes spanning tightly coupled areas, choose the area that owns the behavior. Example:
  `feat(host/approval)` may include matching `server/approvals` plumbing if approval policy is
  the reason for the commit.
- For genuinely cross-cutting but single-intent changes, use a broad stable scope such as
  `repo/workspace`, `core/api`, or `product/p0`; do not invent `misc`.

## Subject rules

Subjects must be short, imperative, and specific:

- Keep under 72 characters; aim for 50.
- Use lowercase except proper nouns, protocol names, and identifiers.
- Use imperative mood: `add`, `fix`, `default-deny`, `stream`, `persist`.
- Do not end with punctuation.
- Do not repeat the type or scope.
- Do not use vague words: `stuff`, `misc`, `updates`, `changes`, `wip`.
- Do not mention file names unless the file is the user-visible artifact.

Good:

```text
fix(host/proc): reject disallowed argv before approval
feat(server/sse): emit approval request events
docs(product/memory): clarify profile recall boundaries
```

Bad:

```text
fix: stuff
chore(repo): updates
docs(docs): update docs.md.
feat(tm-server): add changes to src/lib.rs
```

## Body rules

Omit the body when the header explains the change. Add a body only for information a future
maintainer cannot infer from the diff:

- motivation or product invariant;
- migration notes;
- security or approval behavior;
- compatibility risk;
- milestone or parity context;
- why the boring alternative was not enough.

Body style:

- Wrap at 72 characters.
- Use present tense.
- Explain why and observable behavior, not a file-by-file diff.
- Do not paste test logs, changed-file lists, or implementation transcripts.

Example:

```text
fix(sandbox/security): fail closed for unknown resource schemes

Unknown schemes previously reached backend-specific resolution. Reject them
before dispatch so future sandbox backends inherit the same default-deny
boundary.
```

## Footers

Use standard Conventional Commit footers when needed:

```text
BREAKING CHANGE: describe the migration required
Refs: #123
Closes: #123
```

Rules:

- `BREAKING CHANGE:` is required for incompatible public API, CLI, protocol, config, storage,
  or SDK behavior.
- `Refs:` links related work without claiming closure.
- `Closes:` is only for work that fully satisfies the issue.
- Footers are not a substitute for a useful subject.

## Reverts

Use Conventional Commit's revert shape and include the original header when possible:

```text
revert(core/agent-loop): stream tool-call deltas

This reverts commit <sha>.
```

If the revert also fixes forward behavior, prefer a normal `fix(...)` commit that explains the
replacement behavior instead of a raw revert.

## Splitting rules

One commit should represent one reviewable intent.

Split when:

- feature work and unrelated cleanup are mixed;
- documentation updates are not required to understand the behavior change;
- tests are for a different behavior than the code change;
- dependency/build changes are independently meaningful;
- two crates change for unrelated reasons.

Keep together when:

- tests prove the behavior in the same commit;
- docs are required because the behavior or milestone scope changed;
- a crate API change and all callsite migrations are one clean cutover;
- generated lockfile or package output follows the source change.

## Quick chooser

```text
Did behavior change for a user/API/client?       feat or fix
Was broken behavior corrected?                  fix
Only docs changed?                              docs
Only tests changed?                             test
Only formatting changed?                        style
Only structure changed with same behavior?      refactor
Did it reduce allocations/latency/work?         perf
Did dependencies/build/workspace change?        build
Did automation change?                          ci
Is it maintenance with no better type?          chore
Is it undoing a commit?                         revert
```
