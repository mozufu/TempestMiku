# Android on-device ASR benchmark

> P6.6 was explicitly deferred on 2026-07-15. This harness and its raw reports are retained evidence,
> not an active production implementation. See the
> [deferment evidence and resume contract](../../docs/evidence/2026-07-15-p6-6-on-device-asr-deferment.md).

This is the isolated feasibility harness for TempestMiku P6.6. It is a
debug-only Android app with its own package ID,
`org.mozufu.tempestmiku.asrbenchmark`. It does not share the production Miku
application, permissions, storage, session API, or message-send path.

The first baseline is SenseVoice Small INT8 through `sherpa_onnx`. The model is
not committed and is not bundled into the APK. `benchmark.sh prepare` downloads
the pinned upstream archive, checks its exact byte count and SHA-256, and builds
a second manifest containing hashes for the files copied to the device.

- Archive: `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2`
- Bytes: `163002883`
- SHA-256: `7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e`
- Runtime: `sherpa_onnx` `1.13.4`, CPU provider, arm64 only
- Input: mono 16 kHz WAV, non-empty, at most 60 seconds

The upstream download and model layout are documented in the
[sherpa-onnx SenseVoice model guide](https://k2-fsa.github.io/sherpa/onnx/sense-voice/pretrained.html).
SenseVoice weights are governed by the
[FunASR Model Open Source License Agreement 1.1](https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE),
which requires source/author attribution and retention of the model name. The
manifest records that license and the attribution `SenseVoiceSmall by
FunAudioLLM/FunASR`; a production installer must surface both rather than rely
on this developer README.

## Run

Use the repository dev shell so Flutter, Android, `jq`, and related tooling are
consistent:

```sh
nix develop --command tools/android_asr_benchmark/benchmark.sh prepare
nix develop --command tools/android_asr_benchmark/benchmark.sh build
nix develop --command tools/android_asr_benchmark/benchmark.sh install
nix develop --command tools/android_asr_benchmark/benchmark.sh push
nix develop --command tools/android_asr_benchmark/benchmark.sh run
```

Set `ADB_SERIAL` when more than one transport is visible, including a Tailnet
wireless-debugging endpoint:

```sh
ADB_SERIAL=100.64.0.2:37109 \
  nix develop --command tools/android_asr_benchmark/benchmark.sh result
```

Tap **Run benchmark** in the standalone app, then retrieve the JSON report:

```sh
nix develop --command tools/android_asr_benchmark/benchmark.sh result
```

`all` performs the first five setup commands but deliberately does not start
inference without the visible tap. Model and corpus files live only in this
tool's ignored `.asr-benchmark/` directory and the benchmark app's private,
backup-excluded Android storage.

## Corpus and gates

`prepare` seeds two upstream sanity WAVs and matching reference transcripts in
`.asr-benchmark/corpus/`. Add one UTF-8 `.txt` reference beside each additional
`.wav` using the same basename. The P6.6 acceptance corpus requires at least 50
Taiwan Mandarin and code-switch items, including at least three consecutive
59–60 second inputs.

On macOS, generate the deterministic first-pass Taiwan Mandarin corpus with:

```sh
tools/android_asr_benchmark/generate-zh-tw-corpus.sh
```

This renders the checked-in 50-item manifest with Apple's `Meijia` `zh_TW`
voice, including three 59.5-second sustained-speech inputs. It is explicitly a
`synthetic_tts` corpus: it exercises Traditional Chinese references,
Taiwan-local wording, Mandarin/English code switching, input format, sustained
runtime, and reproducibility. It does **not** establish accuracy for real
speakers, microphones, background noise, Taiwanese accents, or disfluencies.
Those require a separately consented human-speech corpus before P6.6 closes.

The report records model load time, per-file inference time, real-time factor,
recognized language/emotion/event, CER where a reference exists, and process
RSS/high-water RSS. It passes only when all current feasibility gates hold:

- model plus tokens are at most 350 MiB;
- peak RSS is at most 1 GiB;
- every inference finishes within 45 seconds;
- maximum real-time factor is at most 0.5;
- the corpus has at least 50 items and at least three long runs.

CER is strict after only case, whitespace, and punctuation normalization.
Traditional/Simplified substitutions remain errors on purpose, so a model that
recognizes the words but emits Simplified Chinese cannot masquerade as a
Taiwan-ready text surface.

The checked-in `evidence/2026-07-15-2412dpc0ag-sanity.json` result is intentionally
`passed: false`: its two upstream fixtures establish the initial runtime baseline.
`evidence/2026-07-15-2412dpc0ag-zh-tw-synthetic.json` is the expanded physical run:

- 52 total cases: 50 synthetic `zh_TW` items plus two upstream fixtures;
- three long inputs at 59.50–59.51 seconds;
- 1,763 ms model load, 972,924 KiB peak RSS, 3,133 ms maximum inference, and
  0.05265 maximum RTF;
- all current resource/runtime gates passed;
- 0.3726 strict CER across the 50 Traditional references.

The resource result is strong but the RSS headroom is only 75,652 KiB below the
1 GiB gate. The strict CER is deliberately not a pass criterion yet: the report
shows Simplified Chinese output, numeric ITN differences, and weak English
proper nouns. Freeze the Taiwan-ready text-quality contract and run a consented
human-speech corpus before treating the engine as shippable.

The NVIDIA Parakeet zh-TW model remains a comparison candidate, not an implicit
fallback. It can replace this baseline only after portable weight provenance,
redistribution/licensing, conversion reproducibility, and the same physical
device gates are all documented.
