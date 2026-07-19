# P7.2b approval-backed persona addenda closeout evidence

Date: 2026-07-18
Base revision: `27fac31` (`main`, before this closeout work)
Scope: Auto-mode candidate detection, durable manual review, immutable activation/rollback, replay,
and affected clients.

## Outcome

P7.2b is closed. Auto mode can create a bounded persona-addendum proposal when the current owner
message is corroborated by active P8 evidence. It cannot approve or activate that proposal. The
existing durable manual approval effect remains the only activation path, and rollback is a second
manual approval.

The active addendum can change only typed tone, address, or interaction-preference guidance. It
cannot change `SOUL.md`, identity, safety instructions, modes, voice caps, scopes, route triggers,
capabilities, source, configuration, or deployment.

## Candidate contract

- Only unlocked Auto-mode turns at the `moderate` evolution tier are eligible. Detection runs after
  the durable turn completes and after its runtime state is promoted; failed, cancelled, locked-mode,
  and lower-tier turns do not propose.
- The current message must match one of the small typed preference classes: form of address,
  Traditional Chinese, concise/detailed replies, outcome-first replies, fewer questions, or
  direct/gentle tone.
- A repeated preference needs two distinct active, included P8 records with evidence. An explicit
  persona correction needs one. Legacy lexical recall, excluded/tombstoned records, missing
  evidence, and a record absent from the bounded hybrid result cannot qualify.
- Detection inspects at most five retrieval candidates, stores at most three evidence records and
  four bounded redacted references per record, and never copies full memory records into events or
  approval labels.
- A normalized signal produces a stable SHA-256 dedupe key. A pending or approved candidate blocks
  duplicates; terminal deny, timeout, failure, and rollback outcomes start a seven-day cooldown.
- The proposal records the source turn, trigger, evidence count, stable base profile digest, typed
  change, and immutable candidate digest. It uses the same stale-base/pointer checks as a manual
  persona proposal.

## Durable concurrency finding

The first real PostgreSQL run exposed a cross-instance race in the candidate dedupe path: placing
`pg_advisory_xact_lock` inside the same statement as the duplicate read allowed that statement's
snapshot to predate the lock, so two server instances could both insert.

The final implementation acquires the transaction-scoped advisory lock in its own statement, then
performs the duplicate/cooldown read and insert under a fresh `READ COMMITTED` statement snapshot.
The two-instance regression now produces exactly one proposal.

## Crash atomicity and retry finding

An independent audit found two crash windows after the initial closeout:

1. the managed persona/mode file pointer could be replaced before the approval effect and its
   terminal `write_proposal` event committed, so a retry saw its own target as stale; and
2. Auto mode could commit a pending proposal before creating its durable approval and events,
   leaving a dedupe-blocking proposal that the owner could never review.

Managed persona and mode installs now treat an already-active target as a successful replay only
when the immutable manifest exactly matches the requested digest, base, changes, and source
proposal. Rollbacks persist the source digest in the atomic active pointer, so a retry to either a
prior version or the hand-authored base succeeds only for the identical transition. A different
source digest, target, manifest, or provenance still fails as stale.

The approval-effect regression injects failure after the file mutation but before effect/event
finalization. The retry reuses the exact approved payload, leaves one immutable version, advances
the proposal/effect consistently, and commits exactly one terminal proposal or rollback event.

Auto proposal creation now uses one Store bundle. The proposal, approval request, blocked effect,
awaiting-approval audit, pending `write_proposal`, `approval` event, and request-event linkage commit
in one in-memory critical section or one PostgreSQL transaction. Local publication happens only
after that commit; replay remains the source of truth if publication is interrupted. A PostgreSQL
trigger that aborts the approval insert proves the earlier proposal insert rolls back. Two server
instances and a restarted third instance then prove one proposal, one approval/effect, and one of
each event, while preserving the existing dedupe/cooldown contract and manual-only activation.

## Public workflow proof

The deterministic `tm-e2e` `evolution-policy` recording exercises the public session API and proves:

1. a repeated, P8-backed preference creates one pending Auto proposal with bounded provenance;
2. no active persona changes before approval, including across a server recovery boundary;
3. manual denial changes review state only and starts the cooldown;
4. the same candidate is suppressed during cooldown;
5. a distinct candidate can be manually approved and is composed on the next turn;
6. `SOUL.md`, mode definitions, safety and capability envelopes remain unchanged; and
7. a separate manual rollback restores the hand-authored base.

The generated local recording passed at
`/private/tmp/tm-evolution-policy-20260718/{manifest.json,report.md,index.html}`. This path is an
ephemeral test artifact; the durable contract and exact commands are preserved here.

## Verification matrix

| Gate | Command | Result |
|---|---|---|
| Detector and durable in-memory lifecycle | `cargo test -p tm-server persona_candidate -- --test-threads=1` | includes atomic bundle fault rollback and manual-only dispatcher coverage; passed |
| Manual lifecycle/stale/default-deny coverage | `cargo test -p tm-server api::tests::evolution_review -- --test-threads=1` | includes filesystem-before-finalization activation/rollback retry; passed |
| Persona/mode filesystem transition replay | `cargo test -p tm-modes --lib` | 33 passed |
| Real PostgreSQL cross-instance dedupe | `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=<disposable-p7-dsn> cargo test -p tm-server gated_postgres_auto_persona_dedupe_and_cooldown_are_cross_instance_atomic -- --test-threads=1 --nocapture` | 1 passed |
| Real PostgreSQL bundle rollback/restart | `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=<disposable-p7-dsn> cargo test -p tm-server gated_postgres_auto_bundle_rolls_back_crash_and_restarts_without_duplicates -- --test-threads=1 --nocapture` | 1 passed; trigger rollback, two instances, restart, exact events |
| Real PostgreSQL migration/restart/resolve-once | `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=<disposable-p7-dsn> cargo test -p tm-server gated_postgres_review_migrates_restarts_and_resolves_once -- --test-threads=1 --nocapture` | 1 passed |
| Public API/evidence workflow | `cargo run -p tm-e2e -- record evolution-policy --output-dir /tmp/tm-evolution-policy-20260718` | passed |
| Flutter parser and review UI | `nix develop --command flutter test test/widget_test.dart` from `clients/miku_flutter` | 45 passed |
| Focused strict Rust lint | `cargo clippy -p tm-modes -p tm-server --all-targets --all-features -- -D warnings` | passed |
| Formatting and whitespace | `cargo fmt --all --check` and `git diff --check` | passed |

The final workspace-wide formatting, Clippy, and test gates are recorded by the encompassing
roadmap closeout; they are not duplicated as P7-specific claims here.
