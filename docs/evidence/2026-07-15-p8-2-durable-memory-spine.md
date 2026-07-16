# P8.2 durable scoped memory spine evidence

Decision date: **2026-07-15** (`Asia/Taipei`).

Status: **P8.2 CLOSED — durable storage boundary complete; P8.3 local embeddings and hybrid
retrieval remain open.**

P8.2 makes the P8.1 record/provenance contracts durable without changing the current lexical
recall control group. It does not add pgvector, vector data, an embedding provider, hybrid ranking,
or a new turn path.

## Durable schema and migration contract

Ordered, checksummed migration `0014_p8_durable_memory.sql` adds:

- `memory_records` for schema-versioned episodic and semantic records, owner/scope authority,
  lifecycle status, correction/supersession links, and deterministic content/version keys;
- normalized `memory_record_evidence` and `memory_record_relations` projections;
- versioned `memory_embedding_provenance` plus deduplicated, cancellable
  `memory_embedding_jobs`; and
- `memory_scope_tombstones` and `memory_legacy_migration_quarantine` for revocation and safe
  legacy migration diagnostics.

New typed records derive a SHA-256 `memory-content-v1-*` key from the stable logical payload
(kind, owner, scope, and episodic text or semantic triple), independently of a generated UUID or
mutable lifecycle fields. They also derive a state-bearing `memory-record-v1:*` version key; its
immutable `created_at` field is deliberately excluded so an idempotent update preserves creation
time while retaining a valid version key.
Independent retries with new UUIDs therefore resolve to the first record with the same scoped
content; a changed observation retains its own version. Legacy rows keep deterministic mirror keys
from their row identity and replay through their primary keys.

`profile_facts`, `recall_chunks`, and `memory_summaries` are mirrored through after-insert/update
triggers. The migration replays existing rows through those same triggers, quarantines only invalid
legacy rows, and leaves the legacy tables and their lexical query untouched. This preserves P8.1 as
the control group until P8.3 deliberately changes candidate generation.

## Authority, correction, and revocation contract

The durable store always selects a record with both `owner_subject` and `memory_scope`; a mismatched
owner/scope is not found. Correction/supersession links must resolve within that same authority.
`active_memory_records` filters before any future ranking: only an active, currently effective
successor can suppress its predecessor. A candidate, withheld, unsupported, or otherwise inactive
correction cannot hide existing supported memory.

Project unlink/revocation writes a durable scope tombstone from `drive_links`, immediately makes
future record/job reads and writes not-found for that owner/scope, and cancels queued or running
embedding jobs. Tombstones survive a server restart. This is a durable denial; no process-local
fallback authority or write path is introduced.

## Readiness contract

`PostgresStore::memory_readiness` validates the P8.2 migration, required relations, and required
column shape. `RuntimeStatus` exposes the resulting schema/vector/embedding state, and a durable
server fails startup and `/ready` fails closed when the durable schema is corrupt.

The state deliberately distinguishes:

- schema `ready` vs `corrupt`;
- pgvector `disabled`, `missing`, `ready`, or `unsupported_dimensions`; and
- embeddings `disabled`, `ready`, or `temporarily_unavailable`.

With the default disabled provider, a missing pgvector installation does not make lexical durable
storage unavailable. With an enabled/pinned configuration, P8.2 reports the missing/unsupported or
temporarily unavailable state rather than silently enabling a provider. P8.3 owns provider startup,
vector storage/index creation, and any transition to `ready` dense retrieval.

## Reproduction and passed gates

The PostgreSQL gate creates isolated schemas and cleans them after every test. Against PostgreSQL
16, run:

```sh
TM_POSTGRES_TESTS=1 \
TM_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/tempestmiku_test \
cargo test -p tm-server gated_postgres_p8_durable -- --nocapture

TM_POSTGRES_TESTS=1 \
TM_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/tempestmiku_test \
cargo test -p tm-server \
  gated_postgres_p8_1_lexical_baseline_is_replayable_from_clean_schema -- --nocapture
```

The P8.2 gate proves a clean-schema migration, an upgrade from the P8.1 base schema with legacy
fact/chunk/summary mirroring, unmodified lexical result, owner/scope denial, active-only correction
filtering, UUID-independent content-key replay, embedding-job deduplication, unlink cancellation,
restart retention, corrupt-schema readiness, and a deliberately malformed migration that rolls back
only version 14. `tm-memory` unit tests cover deterministic key construction and all readiness
categories; in-memory store tests cover the same correction, authority, and tombstone semantics.

The P8.1 PostgreSQL replay also passes after migration 0014, retaining its frozen lexical rankings,
metrics, prompt budget, and latency acceptance. Its recorded metrics remain in
[`P8.1 recall baseline evidence`](2026-07-15-p8-1-recall-baseline.md).

## Explicit boundary

P8.2 is a storage and safety spine only. It does **not** claim relevance improvement, pgvector
availability on lumo, local-model provenance, successful embeddings, re-embedding coverage,
lexical/dense fusion, or turn/resource integration. Those are P8.3–P8.5 acceptance work; P8 itself
remains active and P7.2b remains blocked on the later provenance/correction/relevance gates.
