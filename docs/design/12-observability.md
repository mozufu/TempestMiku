# 12. Observability & reproducibility

- **Tracing:** `tracing` spans — `turn` → `cell` → `host_call` — with token counts, bytes,
  durations, capability decisions.
- **Transcript:** the full session (messages + code + outputs + artifacts + host-call records)
  is persisted as a structured log.
- **Replay:** a recorded session can be re-run with host calls served from the log (deterministic
  for debugging) or live (to test prompt/model changes against real capabilities).
