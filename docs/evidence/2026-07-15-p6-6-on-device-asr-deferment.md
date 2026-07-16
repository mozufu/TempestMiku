# P6.6 on-device ASR deferment evidence

Decision date: **2026-07-15** (`Asia/Taipei`).

Status: **DEFERRED — feasible benchmark, incomplete product milestone**.

The owner explicitly deferred P6.6 after the feasibility and Taiwan Mandarin benchmark work. This
is a sequencing decision, not acceptance: P6.1–P6.5 remain closed, P6.6 remains incomplete, and P6
is not marked fully closed. P8 fuller memory subsequently closed; P7.2b is now active. Resume P6.6 only after an
explicit owner decision.

## Scope preserved at deferment

- The production client has only the authority-free [`LocalAsrEngine` contract](../../clients/miku_flutter/lib/asr/local_asr_engine.dart)
  and its deterministic boundary tests. There is no production engine implementation, microphone
  flow, model installer, transcript review integration, or new send path.
- The isolated [`android_asr_benchmark`](../../tools/android_asr_benchmark/README.md) app remains a
  separate debug-only package, `org.mozufu.tempestmiku.asrbenchmark`. It has no microphone or
  network permission, cannot build a release APK, uses only `arm64-v8a`, stores test data in its own
  backup-excluded app-private directory, and does not share production credentials or sessions.
- Model weights and generated WAV files remain ignored local cache data. The checked-in assets are
  the harness, corpus source manifest, and machine-readable result reports.

## Pinned model and runtime

| Field | Recorded value |
|---|---|
| Model | `sensevoice-small-int8-2024-07-17` |
| Runtime | `sherpa_onnx` `1.13.4`, CPU provider, 4 threads |
| Model + tokens | 239,549,735 bytes |
| Model SHA-256 | `c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51` |
| Tokens SHA-256 | `f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc` |
| Source archive SHA-256 | `7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e` |
| License recorded by manifest | FunASR Model Open Source License Agreement 1.1 |
| Attribution | `SenseVoiceSmall by FunAudioLLM/FunASR` |

SenseVoice is a benchmark baseline only. No production model was selected before deferment. NVIDIA
Parakeet zh-TW also remains an unproven comparison candidate: portable weights, redistribution,
conversion reproducibility, Android packaging, and equivalent device gates were not established.

## Physical test target and corpus

| Field | Recorded value |
|---|---|
| Device | Xiaomi `2412DPC0AG` / `rodin` |
| OS | Android 15, SDK 35 (`AP3A.240905.015.A2`) |
| ABI | arm64-v8a |
| Corpus | `zh-tw-meijia-synthetic-v1` |
| Corpus source | [`zh-tw-synthetic-v1.tsv`](../../tools/android_asr_benchmark/corpus/zh-tw-synthetic-v1.tsv) |
| Voice | Apple `Meijia`, `zh_TW`, 185 words/minute |
| Cases | 50 synthetic Taiwan Mandarin/code-switch items + 2 upstream fixtures |
| Long inputs | 3 inputs at 59.50–59.51 seconds |

The synthetic corpus is a deterministic performance, format, orthography, ITN, and code-switch
probe. It is not evidence for microphones, environmental noise, spontaneous speech, disfluencies,
speaker diversity, or Taiwanese-accent accuracy.

## Physical benchmark results

The expanded report passed every resource/runtime gate implemented by the isolated harness:

| Gate | Limit | Result | Status |
|---|---:|---:|---|
| Model + tokens | ≤ 367,001,600 bytes | 239,549,735 bytes | pass |
| Peak process RSS | ≤ 1,048,576 KiB | 972,924 KiB | pass, 75,652 KiB headroom |
| Maximum inference | ≤ 45,000 ms | 3,133 ms | pass |
| Maximum real-time factor | ≤ 0.5 | 0.052647 | pass |
| Corpus size | ≥ 50 | 52 | pass |
| 59–60 second runs | ≥ 3 | 3 | pass |

Model load took 1,763 ms. The run finished without a crash or thermal abort, but no temperature or
sustained multi-run thermal envelope was measured. Peak RSS consumed 92.79% of the 1 GiB gate, so
the remaining memory margin is a resumption risk rather than a production endorsement.

The earlier two-fixture sanity run recorded 1,503 ms load, 724,160 KiB peak RSS, 175 ms maximum
inference, 0.02484 maximum RTF, and 0.0 CER on its upstream Mandarin and English files. Its report is
intentionally `passed: false` because it predates the required corpus-size and long-input evidence.

## Taiwan Mandarin quality findings

- Mean CER across all 52 expanded-run cases was `0.3582958301`.
- Strict CER across the 50 Traditional-Chinese synthetic references was `0.3726276633`.
- A host-side diagnostic that first converted Traditional references with OpenCC `tw2s.json`, then
  reused the same case/whitespace/punctuation CER rules, produced `0.1428204942`. This is diagnostic
  only; it is not part of the on-device report or an acceptance gate.
- The remaining errors concentrate around numeric inverse text normalization and English/code-switch
  terms. Examples include spoken Chinese numerals emitted as digits, `pull request` becoming
  `poolre`, `timeout` becoming `time mode`, and weak recognition of product names.
- SenseVoice emits Simplified Chinese by default in this route. A future production path must make
  Traditional output and ITN policy explicit rather than hiding those substitutions in the gate.

The runtime is feasible, but the collected evidence does not prove that SenseVoice is a
Taiwan-ready production transcription engine.

## Wireless ADB verification

The Android run was installed and driven through Android TLS wireless debugging; no USB serial was
present. Tailnet reachability was then restored and the same report was read back through an
explicit Tailnet ADB selector:

```text
adb connect 100.114.232.3:37109
adb -s 100.114.232.3:37109 shell 'getprop ro.product.model; echo tailnet-adb-ok'
2412DPC0AG
tailnet-adb-ok
```

The report read through that transport had SHA-256
`a5b37932151e6bc1b1fd3fdcda9b1711c69f3b5f73ec2cbfe40389496f343a6b`. The Tailnet IP is stable for
the enrolled device, but Android's TLS connect port may rotate whenever Wireless Debugging restarts;
future work must re-read `adb mdns services` and substitute the current port.

## Evidence files and integrity

| Artifact | SHA-256 |
|---|---|
| [`2026-07-15-2412dpc0ag-sanity.json`](../../tools/android_asr_benchmark/evidence/2026-07-15-2412dpc0ag-sanity.json) | `46b7a8196e230e649711718e4a1800ead037bda0fb0a7de8c0c4059c071058e6` |
| [`2026-07-15-2412dpc0ag-zh-tw-synthetic.json`](../../tools/android_asr_benchmark/evidence/2026-07-15-2412dpc0ag-zh-tw-synthetic.json) | `a5b37932151e6bc1b1fd3fdcda9b1711c69f3b5f73ec2cbfe40389496f343a6b` |
| [`zh-tw-synthetic-v1.tsv`](../../tools/android_asr_benchmark/corpus/zh-tw-synthetic-v1.tsv) | `424fc2e9baf3f40240f7e23ffc7aacd1f00f71a4780074b82c153560873fd621` |

## Gates still open

- Freeze a Taiwan-ready accuracy contract covering Traditional output, ITN display policy,
  code-switch/proper nouns, aggregate error thresholds, and worst-case handling.
- Run a separately consented real-speaker corpus across microphones, noise, accents, pace,
  disfluencies, and corrections.
- Choose and license a production model; do not assume SenseVoice or NVIDIA Parakeet won by default.
- Provide an explicit model installer with pinned origin, manifest/hash verification, attribution,
  app-private backup-excluded storage, deletion, and corrupt/missing-model degradation.
- Implement visible microphone permission plus record/stop/cancel, a killable worker, bounded timeout,
  explicit retry, and deletion after success/failure/cancel/timeout/process death.
- Present an editable transcript and require explicit current/new-session confirmation before the
  existing durable message path can send anything.
- Prove airplane-mode inference, release packaging/signing, upgrade retention boundaries,
  foreground/background/cold-start behavior, idempotency, and physical exact-once sends.

## Resume contract

While deferred:

- Do not add `RECORD_AUDIO`, model weights, a model installer, or ASR runtime code to the production
  application.
- Do not add a server transcription endpoint, platform recognizer, lumo fallback, cloud fallback,
  background recording, hotword path, or automatic send.
- Do not mark P6.6 or P6 complete, and do not treat the synthetic benchmark as human accuracy proof.
- Preserve the standalone harness and raw reports as reference artifacts; incidental build hygiene
  is allowed, but no scheduled product work resumes from this note alone.

To resume, the owner must explicitly reopen P6.6. The first resumed task is to review this evidence,
freeze the text-quality/real-speech acceptance contract and memory-headroom requirement, then decide
whether to harden SenseVoice, benchmark another portable model, or abandon the on-device route.
