# P8.1 recall baseline and contract evidence

Decision date: **2026-07-15** (`Asia/Taipei`).

Status: **P8.1 CLOSED — baseline and contracts frozen before ranking changes**.

P8.1 establishes the control group for P8. It does not claim that the current lexical path is good
enough, and it deliberately does not add P8.2 tables, pgvector, dense retrieval, ranking changes, or
production embedding calls.

## Frozen inputs

- Evaluator: `p8.1-recall-eval-v1` in `tm-memory` plus the server runner that uses the production
  `Store` queries and `MemoryContext` prompt budgeter.
- Manifest: [`p8_recall_v1/manifest.json`](../../crates/tm-server/tests/fixtures/p8_recall_v1/manifest.json),
  SHA-256 `d8da1bd15234b96c2bea73ed8d1a2d4a2623785970b68427fde674a5a9570903`.
- Cases: nine total — five tuning and four held-out — covering relevant, irrelevant, unsupported,
  stale, corrected, superseded, cross-owner, global, and two project scopes.
- Retrieval: active profile facts followed by scoped `recall_chunks`; PostgreSQL uses `simple`
  `tsvector`/`plainto_tsquery` with the existing `ILIKE` fallback and current importance/recency order.
- Bounds: nDCG/Recall at `k=5`, recall query limit 5, final prompt budget 1,600 estimated tokens,
  seven warm latency samples per case.

## Recorded PostgreSQL 16 baseline

The checked-in machine-readable report is
[`2026-07-15-p8-1-lexical-baseline.json`](2026-07-15-p8-1-lexical-baseline.json).

| Cohort | Cases | Mean nDCG@5 | Mean Recall@5 | Mean prompt tokens | Max prompt tokens | Mean candidates |
|---|---:|---:|---:|---:|---:|---:|
| Tuning | 5 | 0.126186 | 0.200000 | 233.60 | 258 | 5.60 |
| Held-out | 4 | 0.471713 | 0.750000 | 201.75 | 261 | 4.75 |
| Overall | 9 | 0.279754 | 0.444444 | 219.44 | 261 | 5.22 |

The recorded overall latency was 0.837 ms p50 and 1.839 ms p95. The slowest individual case p95
was 3.745 ms. P8 freezes a 50 ms **per-case** p95 ceiling before tuning, leaving CI and host-load
headroom while still detecting a material regression. Schema creation and migrations are excluded
from query latency.

The baseline top-five results contain 10 unsupported, one stale, one corrected, one superseded, and
one authority/scope false inclusion across the nine cases; unsupported-or-stale precision failures
total 11. These are expected control-group findings, not P8 acceptance. In particular, the synthetic
cross-owner case demonstrates that the legacy `recall_chunks` row cannot express `owner_subject`;
it is not evidence of a known live multi-user leak in the documented single-owner deployment. P8.2
must make owner authority a stored and filtered field.

## Frozen acceptance policy

Any P8 hybrid candidate must satisfy all of the following on both the overall and held-out cohorts:

- improve nDCG@5 by at least 10% relative to this lexical baseline (minimum overall `0.307730` and
  held-out `0.518884`), without reducing Recall@5 below overall `0.444444` or held-out `0.750000`;
- include zero unsupported, stale, corrected, superseded, cross-owner, or cross-scope records in the
  final top five;
- stay within the 1,600-token budget and five-item final bound;
- keep every case at or below 50 ms p95 under the same seven-sample harness.

The tuning cohort may diagnose changes, but it cannot substitute for held-out acceptance.

## Record and embedding contracts

`tm-memory` now owns schema-versioned episodic and semantic record shapes. Both carry explicit
`owner_subject` and `memory_scope`, session-event/session-message/resource evidence, confidence,
importance, observed/effective timestamps, status, and correction/supersession links. A tagged
`MemoryRecordResource` freezes the serialized resource envelope. Explicit adapters prove that
records read from both `InMemoryStore` and `PostgresStore` compile and validate through these shapes
without changing the legacy persistence schema in P8.1.

`EmbeddingProvenance` freezes provider, model id, dimensions, normalization, SHA-256 content hash,
deterministically derived embedding version, creation time, re-embedding state, and deterministic
record/content/version job key. Disabled providers cannot claim provenance, and mismatched versions
fail validation.

## Reproduction

Against an empty PostgreSQL 16 test database:

```sh
TM_POSTGRES_TESTS=1 \
TM_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/tempestmiku_test \
cargo test -p tm-server \
  gated_postgres_p8_1_lexical_baseline_is_replayable_from_clean_schema \
  -- --nocapture
```

The test creates a unique isolated schema, applies every ordered migration, seeds the versioned
fixture, replays the current store path, compares all deterministic rankings/metrics/budgets to the
checked-in artifact, checks each p95 latency against 50 ms, and drops the schema. Set
`TM_UPDATE_P8_BASELINE=1` only when intentionally reviewing a new baseline; it prints the candidate
artifact but does not overwrite evidence.

Normal external-service-free coverage also replays the manifest through `InMemoryStore` and checks
the serialized record/evidence adapters. The baseline artifact is PostgreSQL-owned because FTS is
the production lexical path.
