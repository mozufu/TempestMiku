# Android ASR device A/B benchmark

This standalone, debug-only Android app compares TempestMiku's production streaming ASR contract
with one pinned offline Paraformer candidate. Its package ID is
`org.mozufu.tempestmiku.asrbenchmark`; it has no microphone, network, production-storage, session,
or message-send path. Models and WAVs are downloaded to the ignored host cache and copied through
`run-as` into this package's private, backup-excluded storage. Inference starts only after the
visible **Run benchmark** tap.

## Exact engine selectors

`benchmark.sh` accepts either selector as its second argument (or through
`ASR_BENCHMARK_ENGINE`). The default remains `streaming-production`.

### `streaming-production`

- Model: [`csukuangfj/sherpa-onnx-streaming-paraformer-zh`](https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-zh/tree/2a7f71bb58885c1b522ed4e683abd397355d9fc4)
  at commit `2a7f71bb58885c1b522ed4e683abd397355d9fc4`, Apache-2.0.
- Files: `encoder.int8.onnx` (`165462184` bytes,
  `81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a`),
  `decoder.int8.onnx` (`71664561` bytes,
  `f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f`), and
  `tokens.txt` (`75756` bytes,
  `59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6`).
- Runtime: `sherpa_onnx` `1.13.4`, CPU, two threads, Online Paraformer, 16 kHz mono,
  1,600-sample chunks, 16,000 local zero-tail samples, at most 20,000 decode steps, greedy search,
  endpoint detection disabled.

### `offline-paraformer-candidate`

- Model: [`csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14`](https://huggingface.co/csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14/tree/def027084691107096b5ebba69785756d63de6c5)
  at commit `def027084691107096b5ebba69785756d63de6c5`, whose model card declares
  Apache-2.0.
- Files: `model.int8.onnx` (`243371218` bytes,
  `f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945`) and the same pinned
  `tokens.txt` (`75756` bytes,
  `59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6`). Total model material is
  `243446974` bytes.
- Runtime: `sherpa_onnx` `1.13.4`, CPU, two threads, Offline Paraformer, 16 kHz mono, greedy search.
  The whole waveform is accepted once and `decode` runs exactly once only after the complete input
  is available; no streaming partials or zero tail are used.

Both engines apply Android ICU `Simplified-Traditional` conversion on Android 10 or newer. The
model manifest is fail-closed over the selector, contract, revision URL, commit, license,
attribution, every file size/digest, runtime, inference mode, and conversion settings.

## Run one engine

Use the repository dev shell. Generate the shared schema-3 50-item corpus once:

```sh
nix develop --command tools/android_asr_benchmark/generate-zh-tw-corpus.sh
```

The single setup command for the offline candidate is:

```sh
ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh all offline-paraformer-candidate
```

`all` builds the isolated arm64 debug APK, checks that it contains no model/audio payload, installs
it, downloads and verifies only the selected model in the ignored cache, pushes that model plus the
shared corpus into app-private storage, and opens the app. It deliberately does not bypass the
visible tap. Tap **Run benchmark**, then retrieve the JSON:

```sh
ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh result offline-paraformer-candidate
```

`result` preserves a failed report for diagnosis but rejects a stale report from another selector.
For acceptance, use the fail-closed verifier and save its exact JSON:

```sh
ADB_SERIAL=<serial> nix develop --command \
  tools/android_asr_benchmark/benchmark.sh verify-result \
  offline-paraformer-candidate >target/offline-paraformer-android.json
```

`verify-result` runs a Dart verifier on the host rather than trusting the device's `passed`,
`gates`, or aggregate metrics. It re-hashes the local corpus manifest and every WAV/reference,
re-hashes the local benchmark APK, checks those hashes against the device report, independently
recomputes every case CER/coverage/RTF and every aggregate numeric gate, requires positive measured
RSS, and requires both thermal snapshots for every completed case. Any mismatch exits non-zero.

Replace the selector with `streaming-production` for the control run. Each `push` clears the prior
private benchmark directory, so the report can never accidentally pair one engine's manifest with
the other engine's weights.

After saving both verified reports, emit the machine-readable A/B envelope:

```sh
nix develop --command tools/android_asr_benchmark/benchmark.sh verify-pair \
  target/streaming-paraformer-android.json \
  target/offline-paraformer-android.json \
  >target/android-asr-ab.json
```

The output schema is `tempestmiku.p6-6.android-asr-ab.v1`. The comparator reruns both independent
verifications and rejects anything except the exact production and candidate selectors, the same
persisted benchmark-installation UUID, physical device/build identity, local APK digest, and local
corpus digest. Reinstalling with cleared app data intentionally breaks the pair identity.

Individual commands remain available:

```sh
nix develop --command tools/android_asr_benchmark/benchmark.sh prepare streaming-production
nix develop --command tools/android_asr_benchmark/benchmark.sh verify-corpus
nix develop --command tools/android_asr_benchmark/benchmark.sh build
nix develop --command tools/android_asr_benchmark/benchmark.sh verify-apk
nix develop --command tools/android_asr_benchmark/benchmark.sh install
nix develop --command tools/android_asr_benchmark/benchmark.sh push streaming-production
nix develop --command tools/android_asr_benchmark/benchmark.sh run
nix develop --command tools/android_asr_benchmark/benchmark.sh verify-result streaming-production
```

## Storage and bundle boundary

No model or audio is committed or declared as a Flutter asset. Host-only data stays below ignored
`.asr-benchmark/`:

- selected revision directory containing only its verified model files;
- `manifests/<engine>.json`;
- `corpus-manifest.json`;
- `corpus/*.wav` and matching references.

The selected files are copied to `files/asr-benchmark/` inside the benchmark package. The APK build
then fails if its ZIP entries contain an ONNX model, token file, model manifest, or common audio
payload extension. This check supplements the package's existing debug-only, arm64-only, no-network,
no-microphone boundary.

## Corpus, report, and gates

The schema-3 corpus manifest names the exact 50 WAV/reference pairs, categories, and expected long
runs, and binds every WAV/reference by byte length and SHA-256. It also pins the checked-in source
inventory digest; the final three cases are the required consecutive long runs. Schema 1 remains
readable only for the historical inventory, and the older unbound schema 2 must be regenerated
before it can produce current evidence. The same corpus and the same quality/resource gates apply
to both selectors:

- execution must be a physical arm64 Android process with build fingerprint and device metadata;
  host/emulator results fail
  `android_device_execution` and never qualify as device evidence;
- exact model material is at most 350 MiB and the selected runtime/manifest contract matches;
- peak RSS must be a positive measurement and at most 1 GiB, every item is at most 45 seconds,
  maximum RTF is at most `0.5`, and Android thermal status must be present before and after every
  completed case;
- at least 50 items complete with no failed or empty raw/converted output;
- all three 59–60 second inputs complete with at least `0.75` full-reference LCS coverage and
  `0.60` incremental final-third LCS coverage;
- converted mean CER is at most `0.12`, and code-switch converted mean CER is at most `0.25`.

Each schema-3 report records `engine`, `evidence_scope: android_device`,
`host_results_qualify_as_device_evidence: false`, exact model/revision/file digests, inference mode,
runtime build, device fingerprint/ABI identity, a persisted benchmark-installation UUID, installed
benchmark APK digest, the corpus-manifest digest, RSS, thermal snapshots, RTF, raw/converted text,
CER, both long-run coverage metrics, and all gate failures. Use `verify-pair` rather than manually
matching a subset of these identities. A passing synthetic report is an Android-device
orthography/performance result only; it does not replace consented real-speaker or production-client
lifecycle evidence.

The app writes a non-passing `benchmark_interrupted` report before it launches
the worker isolate, so a crash or process exit cannot leave an older passing
report behind. Model/setup phases have bounded watchdogs. Each synchronous
native decode has a 45,000 ms wall watchdog owned by the UI isolate; if it
expires, the app first atomically flushes a `native_decode_watchdog` failure
report with the case filename/category and then kills the standalone benchmark
process. A Dart `Future.timeout` or worker-isolate cancellation alone is not a
native-call bound, which is why process termination is deliberate. Reopen the
app and retrieve `last-result.json` for diagnosis; a watchdog report can never
pass host verification.

## Consented real-speaker text scoring

The repository scorer does not record or access a microphone. After a speaker explicitly consents
and the exact intended sentence is captured separately for each voluntary production-app attempt,
place the reference and unedited Android draft in an ignored local JSON file. The strict schema
requires the final signed APK hash supplied independently by the caller, retained release
certificate, device fingerprint, no audio upload/retention, 10–100 unique item ids, unique UUIDv4
capture ids generated randomly for the individual attempts, unique non-placeholder exact references,
an explicit nullable `qualityIssue` for every capture, and coverage for `quiet`, `noisy`, `normal-pace`, `fast`,
`disfluent-corrected`, `taiwan-local`, `numeric`, `proper-name`, and `code-switch`.
Reference uniqueness and the six-character meaningful-content minimum use the
same case/whitespace/punctuation normalization as CER. Punctuation-only,
short filler, `umm`/`uh`/`hmm`-like, and Chinese filler-only references are
rejected rather than being presented as accuracy evidence.

Copy `real-speaker-input.example.json` into `.asr-benchmark/`; the checked-in example intentionally
has consent/reference flags set to false, invalid capture-ID placeholders, and
empty hypotheses, so it cannot accidentally pass as evidence. Replace each
capture placeholder with a newly generated UUIDv4 only after the corresponding
consented capture; do not reuse UUIDs between attempts. Keep item ids opaque
(`speaker-01`, etc.) rather than placing a person's name in them.

```sh
cd tools/android_asr_benchmark
dart run bin/score_real_speaker.dart \
  .asr-benchmark/real-speaker-input.json \
  "$(shasum -a 256 ../../clients/miku_flutter/build/app/outputs/flutter-apk/app-arm64-v8a-release.apk | awk '{print $1}')" \
  .asr-benchmark/real-speaker-report.json
```

The supplied digest must therefore bind the final signed split
`app-arm64-v8a-release.apk`, not a universal APK or an installed package from a different build.

The command exits non-zero unless mean converted CER is at most `0.20`, p90 is at most `0.35`,
code-switch mean is at most `0.30`, and there are no empty, truncated, or aggregate-signal-warning
drafts. The durable v2 report keeps only opaque item ids, category tags,
CER-normalized character counts, CER values, per-item quality flags, aggregate
gates, and the final APK/certificate hashes. It does **not** retain the raw
device fingerprint, capture IDs, reference/hypothesis text, or per-item text
hashes. Instead it binds the complete logical input with one SHA-256 over JSON
whose object keys are recursively sorted. That binding includes every random
capture UUID, raw fingerprint, text, tag, and flag; the undisclosed random UUIDs
prevent ordinary dictionary guessing of an individual sentence. This is an
integrity binding, not encryption or anonymization: anyone who obtains the
owner-local input can recompute it. The ignored input must therefore remain
owner-local and should be deleted under the evidence retention policy after
the accepted aggregate is recorded. A prior attempt without an exact reference
cannot be inserted retroactively, and running this scorer is not consent to
make another recording.

## Local verification without a device

```sh
nix develop --command bash -n tools/android_asr_benchmark/benchmark.sh
nix develop --command bash -n tools/android_asr_benchmark/generate-zh-tw-corpus.sh
nix develop --command tools/android_asr_benchmark/benchmark.sh verify-corpus
nix develop --command bash -lc \
  'cd tools/android_asr_benchmark && flutter analyze && flutter test && flutter build apk --debug --target-platform android-arm64'
nix develop --command tools/android_asr_benchmark/benchmark.sh verify-apk
```

These checks validate source, contracts, and packaging. They intentionally do not mark either
engine's Android benchmark as passed without an actual device run.

The checked-in `evidence/2026-07-15-*.json` files remain historical SenseVoice reports. Their
schema/model do not satisfy either current selector and are not rewritten as current evidence.
