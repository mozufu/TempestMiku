# P9 production egress and opaque-secret closeout evidence

Date: 2026-07-18
Base revision: `27fac31` (`main`, before this roadmap closeout work)
Scope: production HTTP egress, exact turn authority, DNS/IP/redirect policy, budgets, audits,
revocation, opaque secret handles, restart behavior, and the native server/CLI integration.

## Outcome

P9 is closed for the native tm runtime. Production egress is disabled by default and has no ambient
`fetch`, socket, DNS, environment, or secret-value escape hatch. When the owner explicitly enables a
configuration, `http.get`, approval-backed `http.request`, and `secrets.use` all pass through one
shared `tm-egress` runtime and the current invocation's exact grants.

The OMP ACP bridge remains an external, replaceable coding backend. It is not given P9 secret handles
or implicit P9 authority; this closeout applies to the sole tm-lang sandbox and host SDK boundary.

## Authority and transport contract

- `EgressConfig::default()` is disabled. Enabled destinations must name an exact HTTPS scheme, host,
  port, path-prefix set, method set, redirect graph, header allowlist, policy version, and request /
  response / count / time caps. Caller `Authorization`, cookie, proxy, host, content-length, and
  hop-by-hop headers are rejected.
- A call needs the SDK capability plus exact `egress.destination:<id>` authority. A secret handle
  additionally needs exact `secrets.use:<id>` authority. Server turns add those names only to an
  existing mode envelope that already owns network reach; child actors receive them only through
  explicit attenuated delegation. No wildcard grant is generated.
- Each hop resolves DNS once, rejects unexpected ports and non-public/special answers by default,
  pins the validated address set into the HTTP client, disables automatic redirects, and
  re-authorizes every redirect against the configured graph and current exact grants. Link-local and
  cloud-metadata endpoints remain hard denied even under an explicit private-network opt-in.
- Request, response, request-count, and elapsed-time reservations are atomic at both destination and
  session scope. In server mode, `egress_session_usage`, `egress_destination_usage`, and outstanding
  `egress_budget_reservations` are updated in one transaction behind a session-row lock. Active
  sessions retain caps across process restart and concurrent API instances; terminal session cleanup
  may remove only those accounting rows. Response bodies must be bounded UTF-8. The only host-only
  response header currently exposed is one bounded `mcp-session-id`; it is omitted from model-visible
  serialization.
- Non-GET `http.request` calls require approval. Before and after approval, the runtime resolves the
  exact destination and opaque-handle record without reading its environment value. The approval
  action shows bounded redacted query/body semantics, method/origin/path/byte/header-name metadata,
  canonical query/request/target digests, and destination/secret id/version; it excludes header,
  secret, and sensitive query/body values. Any snapshot change fails before DNS.
- Before transport, a host-owned effect-scope id plus canonical session, actor, target, and request
  hashes claim one durable `egress_mutation_effects` row. The state machine is
  `started -> succeeded|failed|uncertain`; terminal intent persistence precedes completion/failure
  audit. Same-intent retries return a hash-only receipt, collisions fail closed, and a persisted
  `started` or `uncertain` effect is never resent after origin loss. This boundary is generic
  `tm-egress` state and has no dependency on `tm-mcp`.

## Secret and restart contract

`secrets.use` returns a random opaque token bound to the current session, actor, secret id/version,
and allowed destination set. The configured environment value is read only at the authorized host
request boundary into zeroizing storage. The host does not intentionally place it in config, the tm
value heap, approval payloads, artifacts, events, or model results. Exact literal occurrences in
response text or exposed host-only metadata are replaced before return. This is deliberately
narrower than an information-flow claim: transformed, encoded, hashed, or summarized reflection is
not detectable by exact-value redaction. Credential-bearing destinations are owner-trusted
endpoints; response redaction is defense in depth and does not make an untrusted authenticated
endpoint safe.

Handles are intentionally process-local and are never rehydrated after restart. A fresh runtime
reloads only trusted policy; an old handle fails before DNS. Unlike handles, active-session usage,
outstanding reservations, and mutation receipts are server-durable. Restart cannot reset caps or
authorize re-execution. Destination/secret revocations and same-version policy replacement retain
denial within an epoch; an explicit version rotation is required to reopen a revoked target, while
destination usage remains keyed by stable destination id so rotation does not reset accounting.

Policy is snapshotted before transport work. This lets an operator revocation/reload complete without
waiting for a slow peer. An already-authorized request may finish against its immutable snapshot, but
every request starting after revocation fails before DNS. Started, completed, failed, and denied
events persist only bounded destination/version/generation, method, byte/count/time, status, redirect,
redaction, and error-code metadata; URLs, queries, bodies, credentials, and secret values are absent.

## Live canary

The opt-in production path used the system resolver, public-address checks, address pinning, TLS,
budgets, and audit sink to fetch `https://example.com/` under the exact
`egress.destination:iana_example` grant. It returned HTTP 200 and both `egress_started` and
`egress_completed` events. The test remains opt-in so normal unit tests never require network.

## Verification matrix

| Gate | Command | Result |
|---|---|---|
| Core denial, policy, DNS/IP, redirects/current-hop accounting, budgets, timeout, approval snapshot/digests, exact-once replay/collision, durable-before-completion-audit ordering, secret scope/redaction, revocation, restart and audit | `cargo test -p tm-egress --lib -- --nocapture` | 24 passed |
| In-memory durable effect/budget state machine | `cargo test -p tm-server --lib store::tests::egress_state::in_memory_egress_effects_and_budgets_are_atomic_and_fail_closed -- --exact --nocapture` | 1 passed |
| Real Postgres cross-instance/restart effect and budget durability | `TM_POSTGRES_TESTS=1 TM_TEST_DATABASE_URL=<disposable-p9-dsn> cargo test -p tm-server --lib store::tests::egress_state::gated_postgres_egress_state_survives_restart_and_serializes_instances -- --exact --nocapture` | 1 passed |
| Real public HTTPS transport | `TM_EGRESS_LIVE_TESTS=1 cargo test -p tm-egress opt_in_live_https_canary_uses_the_production_resolver_and_audit_path -- --test-threads=1 --nocapture` | 1 passed |
| Durable server denial/revocation replay, exact mode grants, and terminal cleanup | `cargo test -p tm-server --lib api::tests::egress -- --nocapture` | 5 passed |
| CLI production catalog/default-deny wiring | `cargo test -p tm-cli tests_egress -- --test-threads=1` | 2 passed |
| Explicit child authority attenuation | `cargo test -p tm-agents child_grants_are_an_explicit_bounded_parent_subset -- --test-threads=1` | 1 passed |
| Registered application handler precedence | `cargo test -p tm-lang application_registered_http_handler_is_not_replaced_by_fixture -- --test-threads=1` | 1 passed |
| Strict egress lint | `cargo clippy -p tm-egress --all-targets --all-features -- -D warnings` | passed, zero warnings |
| Strict server production/test lint | `cargo clippy -p tm-server --lib -- -D warnings` and `cargo clippy -p tm-server --tests -- -D warnings` | passed, zero warnings |
| Workspace formatting and patch whitespace | `cargo fmt --all -- --check` and `git diff --check` | passed |

The encompassing roadmap closeout records final workspace-wide formatting, Clippy, tests, and client
gates after all concurrent milestone changes are integrated.
