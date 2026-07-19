# P2 voice and bounded dialectic evidence (2026-07-18)

## Proven software boundary

- `tm-memory` plans a redacted, digest-bound user-model synthesis on every third durable user turn,
  only when approved profile facts exist. Serious, engineering, coding, and scheduler turns do not
  run it. Input is capped at five facts; output is capped at 800 Unicode characters.
- `tm-server` runs the pass with no tools, persists a confirmed `memory_dialectic` trace before the
  main model call, and reuses that exact trace on a durable retry. Empty or unavailable passes are
  durably recorded and fail open without retrying the optional role. Applied synthesis is JSON-quoted
  in the user-data channel and explicitly marked untrusted; it never becomes a system instruction.
- `tm-modes` owns a deterministic P2 rubric for General, negative-state grounding, and Serious
  outputs. The eight-case frozen fixture has both positive and negative calibration examples for
  every scenario. It calibrates the evaluator; it is not evidence of current model output.
- `tm-e2e voice-eval` captures actual final responses from the public session/SSE API, rejects echo
  output, evaluates all three scenarios, and writes exact JSON. The opt-in run against a durable
  `tm-server` `all` role and the configured real `gpt-5.5` model passed all General, grounding, and
  Serious criteria. The exact prompts, finals, session ids, and criterion results are retained in
  [`2026-07-18-p2-voice-live.json`](2026-07-18-p2-voice-live.json).

## Deterministic verification

The focused Rust tests cover cadence/off gates, redaction and Unicode caps, digest validation,
durable failure traces, auxiliary-before-main ordering, confirmed persistence, retry reuse, API mode
selection, frozen voice calibration, and the live-report contract. The integrated workspace gates are
reported separately in `ROADMAP.md`.

## Live acceptance gate

The accepted run used isolated PostgreSQL 16 + pgvector, `TM_SERVER_ROLE=all`, low reasoning effort,
and `OPENAI_CONNECTION_REUSE=0` after the configured OpenAI-compatible proxy repeatedly timed out a
later sequential request 120 seconds after earlier requests succeeded. The client default remains
connection reuse enabled; disabling it is explicit and fail-closed on invalid values. The evaluator
command was:

```sh
TM_P2_VOICE_LIVE=1 TM_MIKU_BASE_URL=http://127.0.0.1:8787 \
  cargo run -p tm-e2e -- voice-eval \
  --output target/tm-e2e/p2-voice-live-latest.json
```

The first complete retained report exposed one real grounding miss: the response offered one small
health-first step but left its duration implicit. The grounding skill now requires the user-visible
reply to state the `<=10 minute` bound, including for seemingly small actions such as water or
breathing. The focused prompt-composition regression passes, and the subsequent exact live report
passes all criteria. The frozen fixture and prompt composition were not substituted for this gate.
