# P8.3 local embeddings and hybrid retrieval evidence

Date: 2026-07-15

## Scope closed

P8.3 adds a bounded `tm-memory` embedding boundary. Enabled configurations pin provider, model,
dimensions, normalization, timeout, batch size, input size, content hash, and derived embedding
version; `disabled` remains the default. The concrete server transport accepts only a loopback HTTP
endpoint with the OpenAI-shaped embeddings response and carries no credential. `openai_compatible`
is intentionally not instantiated before P9 egress and opaque-secret controls.

Migration 0015 records scoped generations and an active-version pointer without trying to install
`pgvector`. When an operator has installed the extension, the server creates a version-derived typed
`vector(N)` table with HNSW cosine index, queues deduplicated records, leases/reclaims batches, and
atomically advances the pointer only when its snapshot coverage is complete. Vector rows carry the
durable content key, so a changed record is excluded from dense recall until it is re-embedded.

Postgres FTS and dense candidates are independently bounded and correction-filtered; `tm-memory`
then fuses them through deterministic RRF (`k=60`) while preserving record/evidence, lexical/dense
rank and score, authority, and embedding version. This is retrieval-only: P8.4 must route it through
turn composition and bounded `memory://` inspection.

## Gates run

| Gate | Result |
|---|---|
| `cargo test -p tm-memory` | Pass — 29 tests, including pinning/hash/normalization and deterministic RRF scope/status filtering. |
| `cargo test -p tm-server --lib` | Pass — 224 tests. |
| Isolated PostgreSQL 16 (`postgres:16-alpine`), `TM_POSTGRES_TESTS=1 ... cargo test -p tm-server gated_postgres_p8 -- --test-threads=1` | Pass — P8.1 replay, migration failure rollback, P8.2 durable spine, and P8.3 lexical-only migration/authority/correction path. |
| Isolated PostgreSQL 16 + pgvector 0.8.5 (`pgvector/pgvector:pg16`, operator-provisioned `CREATE EXTENSION vector`), `TM_POSTGRES_TESTS=1 ... cargo test -p tm-server gated_postgres_p8_hybrid -- --test-threads=1` | Pass — typed vector/HNSW table, lease completion, active pointer, provider-loss retry, partial re-embedding, version switch, stale-content exclusion, correction/scope isolation, and dimension-mismatch lexical fallback. |

## Degradation and authority findings

- Disabled or missing pgvector produces no dense candidates and leaves lexical recall available.
- A provider failure releases the claimed job back to `queued`; the old fully-covered generation
  remains active. A staged partial generation cannot switch the pointer.
- A content change after a job is claimed fails that generation; older dense rows are excluded by
  their content key, so the hybrid result is lexical-only rather than stale.
- Dense candidates require the same server-owned `owner_subject` and `memory_scope` and the same
  active/correction filters as FTS. Non-active, corrected, cross-scope, and tombstoned records never
  reach RRF or later context budgeting.

## Deliberate remaining work

This is not a lumo model-performance or turn-quality acceptance. No production embedding daemon was
configured, and no hybrid result has yet entered a prompt. P8.4 owns turn/resource integration;
P8.5 still requires frozen-fixture relevance improvement, real self-hosted lumo model provenance and
latency/restart proof, full restart/replay evidence, and P8 closeout.
