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
