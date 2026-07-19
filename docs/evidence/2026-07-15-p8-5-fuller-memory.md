# P8.4/P8.5 fuller-memory closeout evidence

Date: 2026-07-15

This note closes P8.4 server integration and P8.5 quality/deployment acceptance. It builds on the
frozen P8.1 control group, the P8.2 durable scoped spine, and the P8.3 local embedding/RRF boundary;
it does not change those control contracts. The machine-readable metrics and runtime provenance are
checked in beside this note as
[`2026-07-15-p8-5-fuller-memory.json`](2026-07-15-p8-5-fuller-memory.json).

## Integrated product path

- `StoreMemoryProvider` now uses the active scoped embedding generation, embeds the turn query
  through the loopback-only local client, asks Postgres for bounded FTS + pgvector candidates, and
  passes the fused result through the existing `MemoryContext` prompt budgeter and turn assembly.
  Disabled embeddings retain the byte-for-byte P8.1 lexical/profile path; provider loss, missing
  generations, and model/dimension mismatch produce a typed `lexical_fallback` trace instead of a
  second loop or a widened grant.
- A hybrid/fallback turn writes one bounded `memory_recall` event before model execution. Concurrent
  API instances converge on the first persisted event, and a retried durable turn reuses that exact
  context rather than querying mutable memory again. The event remains linked to the turn and
  available at `memory://recalls/<turn-id>`; exact records are readable at
  `memory://records/<episodic|semantic>/<record-id>`.
- Recall resources expose record status, correction/supersession links, evidence URIs, lexical and
  dense rank/score, embedding version, RRF score, degraded reason, and prompt-budget inclusion.
  Exact reads and bounded previews/lists use the session's server-owned subject/scope authority and
  return not-found across an authority boundary.
- Approved memory proposals still update the shipped profile/recall views, and now also upsert the
  typed episodic/semantic record with a stable source key and evidence pointing at the exact review
  proposal. Contradictory approved facts preserve and link the superseded typed record. Pending,
  denied, timed-out, unsupported, corrected, superseded, and withheld records never enter recall.
- The supervised worker stages every active scope, embeds only missing/stale content keys, resumes
  expired leases, and can incrementally refresh an already-active model version after a new record
  arrives. The active pointer changes only after current active-record coverage is complete.

The model-visible surface is still the one `execute(code)` tool. P8 adds no ambient `memory.*`
namespace, no chat-native tool, no external memory SaaS, and no `openai_compatible` production path
before P9. The local embedding client also bypasses ambient proxies, refuses redirects, and rejects
credentials, query data, or fragments in the loopback endpoint.

## PR review hardening replay

The current follow-up hardening leaves the frozen P8.1 manifest and thresholds unchanged and
preserves the historical closeout metrics rather than rewriting them; the current hardening replay
is recorded separately below. Ordered migrations 0017–0018 add durable authority guard rows, revocation/write
serialization, scope revisions, monotonic generation activation, pinned dense queries, and input
limit/failure provenance. Approved legacy and typed memory effects now commit atomically. Prompt
admission reserves two profile facts, two ranked recalls, and one recent summary before bounded
refill. The authoritative dense query first materializes owner/scope/status/correction-filtered rows
and then applies exact cosine ordering; the existing HNSW index is retained but is not used to scan a
shared table before authority filtering.

The deterministic replay for this hardening passed on an isolated PostgreSQL 16 + pgvector database:

| Current hardening gate | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| `cargo test --workspace --locked` | Pass |
| `cargo test -p tm-server --locked` | Pass (238 library + 2 binary tests) |
| Clean-schema serial `TM_POSTGRES_TESTS=1` `gated_postgres` suite | Pass (23 tests) |
| Revoked-scope direct database write denial (`TM001`) | Pass |
| Unchanged-stage no-churn and dirty-snapshot lexical fallback | Pass |
| Mixed valid/oversized batch isolation and raised-limit recovery | Pass |
| `cargo test -p tm-e2e --locked` | Pass (14 tests) |
| Playwright Web smoke with isolated durable Postgres | Pass (1; 1 evidence-only test skipped) |
| `nix develop --command flutter test` | Pass (67 tests) |
| Fresh live lumo quality, provider-loss, and resumable re-embedding replay | Pass |
| Fresh separate-process persisted-generation restart replay | Pass |

The fresh live replay used `root@lumo` because lumo's Home Manager/OpenRC deployment is root-owned;
the unqualified SSH target incorrectly selected the workstation user. An SSH loopback forward exposed
only lumo's `127.0.0.1:11434` endpoint at workstation port 11435. With the new 2/2/1 prompt allocation,
overall nDCG@5 was `0.813059`, held-out nDCG@5 was `0.721713`, and both overall and held-out Recall@5
were `1.0`, with zero unsafe inclusions. Overall p95 was `991.343 ms`; the separate restart process
reused the retained generation and reproduced nDCG@5/Recall@5 `0.813059/1.0` with `1387.541 ms`
overall p95. The cold/warm probes were `3865/708 ms`, provider loss returned the expected
`embedding_provider_unavailable` lexical fallback, and an abandoned lease was reclaimed into a ready
1/1 generation. The restart assertion now checks the same frozen baseline-relative acceptance
contract as the primary live gate instead of hard-coding the historical pre-allocation nDCG of `1.0`.

## Historical frozen closeout quality result

The same nine-case P8.1 manifest (`d8da1bd…70903`) was replayed through the real turn provider and
prompt budgeter. The hybrid run used the self-hosted lumo model, not the deterministic test double.

| Cohort | Lexical nDCG@5 | Hybrid nDCG@5 | Relative improvement | Lexical Recall@5 | Hybrid Recall@5 | Hybrid unsafe inclusions |
|---|---:|---:|---:|---:|---:|---:|
| Overall (9) | 0.279754 | 1.000000 | +257.4569% | 0.444444 | 1.000000 | 0 |
| Held-out (4) | 0.471713 | 1.000000 | +111.9933% | 0.750000 | 1.000000 | 0 |

The frozen requirement was at least +10% nDCG@5 with no Recall@5 regression in both overall and
held-out cohorts. All unsupported, stale, corrected, superseded, cross-owner, and cross-scope false
inclusion counts are zero. Final context remained at most 5 records and 382 estimated tokens under
the 1,600-token budget.

For this historical closeout run, the P8.1 50 ms ceiling remains the clean-schema
**lexical/Postgres control** and is not rewritten to
hide local CPU cost. The lumo end-to-end provider run has its own recorded 5,000 ms local ceiling:
overall p50 was 773.211 ms and p95 was 902.987 ms over seven samples per case. Direct lumo cold/warm
probes were 3,765/632 ms; a cold probe after provider restart was 3,857 ms. This keeps real turn cost
visible while staying within the local transport timeout.

## Self-hosted lumo evidence

- Host: Raspberry Pi 5, aarch64, 4 logical CPUs, 7.8 GiB RAM.
- Database: PostgreSQL 17.10 with pgvector 0.8.2 installed in the TempestMiku database.
- Runtime: Ollama 0.32.0, pinned image index
  `sha256:57f573b47f1f71ebb445789f279fe3e596a8beab182f7cf486db9205bad87c5a`.
- Model: `bge-m3:567m`, 566.70M parameters, F16, 1,024 dimensions, MIT license. The Ollama manifest
  hashes to `7907646426070047a77226ac3e684fbbe8410524f7b4a74d02837e43f2146bab`; the 1,157,671,200-byte
  model layer is `sha256:daec91ffb5dd0c27411bd71f29932917c49cf529a641d0168496c3a501e3062c`.
- Isolation proof: the acceptance container ran on a Podman `internal=true` network with
  `OLLAMA_NO_CLOUD=1`; an external TCP probe failed with `Network is unreachable`. The acceptance
  client reached only that container through an SSH loopback forward. The persisted OpenRC service
  is `lumo-tempestmiku-embeddings`, uses the pinned image/model, disables Ollama cloud, and binds
  only `127.0.0.1:11434` for the production `tm-server` loopback client.
- Provider loss/restart: an actual container stop made the endpoint unavailable; the configured
  provider returned `lexical_fallback` with reason `embedding_provider_unavailable`. Before/after
  restart vectors hashed identically to
  `69e273e00c3f7d530932f3e370a0ab14ce4fa4a837f8b29a9aa0ab971a7ccbe5`.
- Resumable re-embedding: one live job lease was deliberately abandoned, reclaimed after expiry by
  another worker, embedded through lumo, and promoted to a `ready` generation with 1/1 coverage.
- Historical closeout server restart: a second Rust test process reconnected to the same isolated Postgres schema and
  active embedding generation. It reproduced nDCG@5/Recall@5 = 1.0, zero unsafe inclusions, the same
  candidate bounds, and 899.478 ms overall p95, then removed the evidence schema.

The checked-in lumo configuration also provisions pgvector, the pinned local embedding service,
and the `TM_MEMORY_EMBEDDING_*` values. This does not enable external model egress or secrets.

## Gates run

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| `cargo test --workspace --locked` | Pass |
| Isolated PostgreSQL 16 + pgvector, full `tm-server` library suite | Pass |
| Live lumo frozen-quality, provider-loss, resumable re-embedding test | Pass |
| Separate-process live lumo/Postgres restart test | Pass |
| `cargo test -p tm-e2e --locked` | Pass |
| Affected Web/Flutter client checks | Pass |
| lumo Home Manager `path:` dry activation | Pass |
| `lumo-tempestmiku-embeddings` OpenRC status + loopback health | Pass |

The detailed commands are intentionally repeatable and opt-in:

```sh
TM_POSTGRES_TESTS=1 \
TM_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:55432/tempestmiku_test \
cargo test -p tm-server --locked -- --test-threads=1

TM_POSTGRES_TESTS=1 \
TM_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:55432/tempestmiku_test \
TM_P8_LIVE_EMBEDDING_ENDPOINT=http://127.0.0.1:11435/v1/embeddings \
TM_P8_LIVE_EMBEDDING_MODEL=bge-m3:567m \
TM_P8_LIVE_EMBEDDING_DIMENSIONS=1024 \
TM_P8_LIVE_SCHEMA=tm_p8_lumo_restart_evidence \
cargo test -p tm-server gated_live_lumo_embedding_replays_frozen_quality_fixture \
  -- --test-threads=1 --nocapture

TM_P8_LIVE_RESTART_SCHEMA=tm_p8_lumo_restart_evidence \
TM_P8_LIVE_CLEANUP=1 \
# plus the same Postgres/local-embedding variables above
cargo test -p tm-server gated_live_lumo_server_restart_reuses_persisted_generation \
  -- --test-threads=1 --nocapture
```

P8 is closed only with this implementation, the frozen/local evidence, and the aligned resource
and deployment contracts together. P7.2b remains manual-approval-only and is the next product slice;
P9 still precedes MCP/live research.
