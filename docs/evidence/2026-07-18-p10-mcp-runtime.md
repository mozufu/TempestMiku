# P10 MCP runtime integration — 2026-07-18

## Scope

This note records the deterministic software proof for selected MCP imports and the live-research
transport boundary, plus an opt-in unauthenticated read-only Internet canary against Cloudflare's
official documentation MCP. It does not claim an authenticated operator service or mutation canary.

TempestMiku imports MCP revision `2025-11-25` through `tm-mcp`. The model-visible surface remains the
single `execute(code)` tool: imported tools and prompts are lazy `mcp.<server>.*` SDK capabilities,
and imported resources use `mcp://` through the existing resource registry.

## Authority and startup contract

- `.tempestmiku/mcp.json` (or `TM_MCP_CONFIG`) is trusted operator configuration. It maps each local
  alias to one exact P9 destination id, an optional opaque-secret id, and an explicit object
  allowlist. It contains no credential values.
- Startup validates that every MCP URL is inside its named HTTPS destination, rejects URL userinfo,
  query strings, and fragments, requires bounded `POST` plus resumable-SSE `GET`, requires
  `Last-Event-ID` and the other Streamable HTTP headers to be allowlisted, and scopes each secret to
  that destination. Query credentials are therefore not an available compatibility path.
- Catalog discovery uses a dedicated host-owned `InvocationCtx` with no actor identity. Every
  request still crosses P9 DNS/IP, redirect, byte/request/time budget, revocation, and secret-handle
  checks.
- Discovery stages the complete bounded catalog and activates it atomically. The production
  server/CLI intentionally keep that generation immutable for the process lifetime; an operator
  config reload restarts the runtime, so cached interpreters cannot retain an old binding digest.
- Only modes that already own `http.get` receive imported authority. Grants are exact imported
  object names plus exact `egress.destination:<id>` / `secrets.use:<id>` references; registration
  alone grants nothing. Child actors must explicitly request the same exact capabilities.
- Mutation annotations come only from local policy. A selected mutation suspends on the existing
  manual approval API before transport. The approval action includes a bounded semantic field/path
  preview while secret-like keys and values are redacted; durable audits retain only input
  digest/size metadata. Denial emits a terminal audit and never calls the peer.

## Durable mutation and replay boundary

Before an approved mutation reaches the peer, the server atomically persists a hash-only intent in
`mcp_mutation_effects`, scoped to the durable session turn, actor, exact server/tool target digest,
and argument digest. Completion is a compare-and-set transition from `started` to `succeeded`,
`failed`, or `uncertain`. A replay of an existing terminal record returns an untrusted replay
receipt and never sends the mutation again. A replay of `started` is conservatively sealed as
`uncertain`, because a crashed process may have reached the peer before persisting its terminal
state.

The stable effect id deliberately excludes catalog generation metadata: an equivalent catalog
reload cannot mint a second remote mutation for the same durable scope/target/arguments. The first
attempt's catalog generation and digest remain in the persisted audit record. In-memory and
Postgres stores compare the remaining identity fields to reject an effect-id/argument-digest
collision, and terminal updates are idempotent only when every terminal field matches.

## Untrusted data, resources, and replay

Remote descriptions and initialize instructions do not become local documentation or system
instructions. Tool, prompt, and resource results are returned as `mcp_untrusted_data` with
`trust: "untrusted"` plus protocol revision, catalog generation/digest, local server alias, object
kind/name, target digest, payload digest, and bounded byte count.

Imported input/output schemas use a strict supported JSON Schema subset. Every input argument is
validated before transport; an advertised output schema requires and validates
`structuredContent`. Annotation text and literal `enum`/`const` values are not disclosed in model
tool documentation, property/required names use a bounded strict ASCII identifier grammar, and an
unsupported keyword rejects the staged catalog without echoing attacker-chosen keyword text.
Remote JSON-RPC tool, prompt, and resource errors expose only a bounded numeric code plus digest in
an explicit untrusted envelope; peer message/data text is never copied into model-facing errors.

The authenticated session resource API dynamically exposes configured `mcp://` resources. Reads
recheck the current mode and exact object/P9 grants, then persist the MCP and egress audit sequence in
the ordinary replayable session event store. The API never exposes the source resource URI in its
audit payload. List and preview operations receive no network or secret grants.

Each generated binding checks one atomically loaded catalog snapshot for generation, catalog
digest, and exact target digest before transport. Streamable HTTP accepts JSON or SSE responses,
tracks bounded SSE event ids/retry values, resumes with `GET` plus `Last-Event-ID`, and rejects
unnegotiated server requests. A session-scoped HTTP 404 clears the stale id and performs a fresh
`2025-11-25` initialize/initialized exchange. Catalog/prompt/resource reads may then retry once, but
`tools/call` never does: even a locally classified read could have remote side effects, so its
current outcome is uncertain and only a future explicit invocation may use the rebuilt session. A
durable mutation is sealed `uncertain`; replay returns `reexecuted: false` without transport. A 404
while resuming an in-flight SSE response is likewise outcome-uncertain and is never blindly retried.

## Deterministic verification

The following commands passed from the repository root:

```text
cargo test -p tm-mcp --no-fail-fast
  21 passed

cargo test -p tm-server mcp --no-fail-fast
  5 passed

TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=<local-test-dsn> \
  cargo test -p tm-server \
  gated_postgres_mcp_mutation_effects_survive_restart_without_reexecution \
  -- --test-threads=1
  1 passed

TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=<local-test-dsn> \
  cargo test -p tm-server \
  gated_postgres_upgrades_legacy_base_schema_without_losing_history \
  -- --test-threads=1
  1 passed

cargo test -p tm-cli -- --test-threads=1
  9 unit tests passed; 4 p0_runtime tests passed

cargo fmt --all --check

cargo clippy -p tm-mcp -p tm-server -p tm-cli --all-targets --all-features -- -D warnings

TM_MCP_LIVE_TESTS=1 cargo test -p tm-mcp \
  live_canary_tests::opt_in_cloudflare_docs_read_only_canary_uses_p9_and_untrusted_envelope \
  -- --exact --test-threads=1 --nocapture
  1 passed
```

The server public-API tests cover exact catalog/P9 turn grants, authenticated `mcp://` list/read,
untrusted provenance, durable requested/completed replay, manual mutation approval, terminal denial,
and proof that a denied mutation never reaches the transport. The store tests cover exact replay,
equivalent-catalog reload, argument-digest collision rejection, terminal CAS idempotency, uncertain
crash recovery, and real-Postgres restart persistence. `tm-mcp` independently covers exact protocol
negotiation, pagination, atomic reload rollback, namespace collision, semantic schema validation,
opaque peer errors, malicious schema-key rejection, equivalent-generation mutation replay,
schema/result/cursor bounds, prompt-injection isolation, resource target binding, Streamable HTTP
JSON/SSE resume and safe session reinitialization, no-retry/uncertain `tools/call`, session
propagation, and P9 audit/grant enforcement.

## Official read-only live canary

The opt-in test selected `https://docs.mcp.cloudflare.com/mcp` as one exact P9 destination and only
the advertised `search_cloudflare_documentation` read tool. It performed the MCP initialize/
initialized exchange, paginated catalog discovery, one read-only call, and returned a bounded
`mcp_untrusted_data` envelope with target/catalog/payload provenance. The P9 sink recorded bounded
transport metadata; no credential or secret handle was configured. The final integrated canary
passed again after the outcome-uncertain/no-resend hardening on 2026-07-18 in 6.83 seconds and
remains excluded from normal network-free tests.

Cloudflare documents this as its official documentation MCP surface in its
[official MCP server repository](https://github.com/cloudflare/mcp-server-cloudflare). The protocol
revision and transport implementation are pinned to the MCP project's
[`2025-11-25` version](https://modelcontextprotocol.io/docs/learn/versioning) and
[Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

This closes P10's selected read-only live-research gate. A future authenticated operator endpoint or
remote mutation remains a deployment-specific canary, not a prerequisite silently inferred from
this unauthenticated read.
