# 12. Observability & reproducibility

- **Tracing:** `tracing` spans — `turn` → `cell` → `host_call` — with token counts, bytes,
  durations, capability decisions.
- **Transcript:** the full session (messages + code + outputs + artifacts + host-call records)
  is persisted as a structured log.
- **Replay:** a recorded session can be re-run with host calls served from the log (deterministic
  for debugging) or live (to test prompt/model changes against real capabilities).
- **Protected readiness/metrics:** `/ready` reports role, Postgres/migration state, worker/drain state,
  and link hydration; `/metrics` reports turn/dream/approval-effect/cron queue depth and oldest age,
  scheduler lag, lease reclaims, heartbeat failures, pending approvals, and link hydration failures.
  `/health` stays public and intentionally reveals only `{ "status": "ok" }`.
- **Redaction boundary:** bearer/pairing/provider credentials and detected secret material are redacted
  before tracing, events, memory, artifacts, cron/approval persistence, or OMP transcript storage.
- **Evolution evidence:** append-only `evolution_audits` records project proposal, approval, and effect
  transitions with typed targets, stable status/error codes, digests, and provenance. Full candidates
  stay behind `memory://evolution-proposals/<id>`, `memory://skill-proposals/<id>`, or
  `memory://review-proposals/<id>`; replay events contain only bounded previews and resource links.
  `cargo run -p tm-e2e -- record evolution-policy` records the public conservative allow/deny,
  Moderate review-only approval, timeout, forged-target, retry, downgrade-policy, spill, and replay
  evidence under `target/tm-e2e`.
