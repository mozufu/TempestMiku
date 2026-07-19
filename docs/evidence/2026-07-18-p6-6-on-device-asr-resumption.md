# P6.6 on-device ASR resumption evidence

Resumption date: **2026-07-18** (`Asia/Taipei`).

Status: **COMPLETE — P6.6 AND FULL P6 CLOSED**.

The owner reopened the whole remaining roadmap on 2026-07-18. This supersedes the scheduling
deferment in the [2026-07-15 evidence note](2026-07-15-p6-6-on-device-asr-deferment.md), but it does
not rewrite that historical evidence and does not turn missing physical proof into a pass. The
production microphone, model installer/runtime, editable review, and explicit send paths are now
implemented. Signed install, exact model activation, offline persistence, consented review-only
recording probes, the latest self-hosted-option install/selection/recording/review/cancel path, and a
same-device synthetic A/B have run. Background/process-death cleanup, a real unavailable-upstream
failure, distinct exact current/new-session sends, and the consented production-local 10-item corpus
also pass. P6.6 and full P6 are closed without treating software or host inference as physical proof.

## Production boundary

- Android requests `RECORD_AUDIO` only from a visible foreground action. Capture is 16 kHz mono
  PCM16, bounded to 60 seconds / 1,920,000 bytes, and written only below `noBackupFilesDir`.
  `onPause`, `onDestroy`, explicit cancel, timeout, and error request cancellation. A process-wide
  retiring handle survives Activity recreation, blocks the next start, performs bounded
  stop/join/interrupt/release/retry cleanup, and remains monitored until the recorder thread actually
  exits. Native and Flutter layers do not turn a failed retirement into cancellation success. Startup
  purges orphaned capture files only after that gate is idle.
- The app never records in the background and adds no hotword, service, cloud transcription,
  platform recognizer, server fallback, or automatic send path.
- Native scratch PCM is deleted before it crosses into inference. The app-owned native capture
  buffer, writable Dart PCM/float copies, and cooperative worker transfer/padding buffers are
  overwritten after use. A platform-owned MethodChannel view is first cloned into that writable
  app-owned buffer; clearing the source view is best effort because Android may expose it as
  immutable, while clearing the owned clone remains deterministic. Model verification/isolate spawn, load, and
  transcription share the same 45-second operation timeout. A synchronous cancellation token prevents
  a pending production spawn from starting an isolate after cancellation; a defensive late-factory
  handler also kills and closes any non-cooperative worker before model load or audio transfer. A
  killable isolate owns one sherpa-onnx recognizer. Normal cancellation cooperatively releases its
  stream and recognizer; a five-second hard kill is the bounded last resort for a wedged native call,
  but explicit overwrite inside that uncooperative native call is not claimed.
- Inference accepts the same 60-second user-audio limit, feeds 0.1-second chunks, adds one local
  second of zero padding to flush Online Paraformer's final tokens, and fails after 45 seconds or
  20,000 decode steps. The padding is not microphone-derived and does not enlarge the capture
  allowance.
- A negative `AudioRecord.read` result before stop/cancel is a terminal capture failure. It is never
  treated as an ordinary EOF that can send partial PCM to the recognizer. After each valid capture
  the client computes only non-replayable aggregate diagnostics: duration, RMS/peak dBFS,
  clipped/near-zero sample ratios, active 20 ms frame ratio, and leading/trailing silence. These
  values can warn about an extremely short, nearly silent, or clipped recording and are visible in
  the editable review sheet; no waveform or frame sequence is retained or uploaded. The same sheet
  shows the installed app id, version/build type, and a streaming SHA-256 of Android's installed
  `base.apk`, or an explicit unavailable state. Fingerprint inspection is independent of capture
  and can never block transcription, review, or send.
- Output is passed through Android ICU's `Simplified-Traditional` transliterator on-device on Android
  10 or newer. It is not assumed equivalent to the host's OpenCC `s2twp` diagnostic, so converted CER
  remains an Android gate. The transcript enters the existing editable import sheet; cancel sends
  nothing, and only an explicit current-session or new-session confirmation enters the existing
  durable message path.

## Selected model and installer

| Field | Production contract |
|---|---|
| Model | `csukuangfj/sherpa-onnx-streaming-paraformer-zh@2a7f71bb58885c1b522ed4e683abd397355d9fc4` |
| Runtime | `sherpa_onnx` `1.13.4`, CPU, 2 threads |
| License | Apache-2.0, shown before explicit owner installation |
| Encoder | 165,462,184 bytes; SHA-256 `81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a` |
| Decoder | 71,664,561 bytes; SHA-256 `f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f` |
| Tokens | 75,756 bytes; SHA-256 `59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6` |
| Total | 237,202,501 bytes, below the 350 MiB gate |

Installation is a separate owner-confirmed drawer action. The installer uses the exact commit-pinned
Hugging Face origin, permits at most one reviewed Hugging Face LFS CDN redirect, disables connection
caching, bounds every file and the total, verifies exact SHA-256 values, writes a synced provenance
manifest, and atomically activates only a fully verified directory. Storage is app-private and
backup-excluded. Missing or corrupt files disable recognition; deletion and interrupted-staging
cleanup are explicit. Model files are not bundled in the APK and there is no fallback model.

SenseVoice remained a feasibility baseline because the physical run reached 972,924 KiB RSS and its
Traditional-reference CER was 0.3726. A small offline Paraformer produced a strong host diagnostic
after conversion but its selected distribution did not provide complete weight-license metadata.
Whisper Tiny was rejected after materially worse Taiwan Mandarin/code-switch quality and long-input
truncation. These comparisons are selection evidence, not production acceptance for the chosen
streaming model.

## Frozen text-quality contract

The editable-draft product contract tolerates visible corrections, but it does not tolerate empty or
silently truncated output. P6.6 closes only when all of the following hold:

- the deterministic 50-item synthetic corpus has converted mean CER at most `0.12`, code-switch mean
  CER at most `0.25`, no empty output, overall normalized-reference LCS coverage at least `0.75`,
  final-third incremental LCS coverage at least `0.60`, and all three 59–60 second runs finish;
- a separately consented real-speaker corpus has converted mean CER at most `0.20`, p90 CER at most
  `0.35`, code-switch mean CER at most `0.30`, no empty/truncated item, and includes quiet/noisy,
  normal/fast, disfluent/corrected, Taiwanese-local, numeric, proper-name, and English-switch speech;
- the UI always labels the result as an editable draft. Numeral/ITN and proper-name ambiguity remain
  visible for review rather than being silently treated as authoritative text.

## Production-inference host reproduction

The production Online Paraformer inference configuration was replayed on an arm64 macOS 26.5 host
against the 50-item deterministic `zh-TW` synthetic corpus. It used two CPU threads, 0.1-second
chunks, and the same one-second tail padding. OpenCC `s2twp` supplies a useful host-side converted
CER diagnostic, but it is not claimed to be phrase-for-phrase equivalent to Android ICU's
`Simplified-Traditional` transliterator; the converted gate must therefore be repeated on Android.

| Metric | Result |
|---|---:|
| Items / 59–60 second runs | 50 / 3 |
| Completed / empty outputs | 50 / 0 |
| Completed long runs / wholesale truncations | 3 / 0 |
| Model load | 0.642 s |
| Mean raw CER | 0.30247 |
| Mean converted CER | **0.08554** |
| Code-switch mean converted CER | **0.19986** |
| Maximum converted CER | 0.5 |
| Maximum inference time | 2.400 s |
| Maximum RTF | 0.05580 |
| Host peak RSS | 537,362,432 bytes |

The exact manifest, model digests, category metrics, environment, and limitations are in
[`2026-07-18-p6-6-streaming-paraformer-host-eval.json`](2026-07-18-p6-6-streaming-paraformer-host-eval.json).
All three long cases consumed the complete waveform plus the one-second tail, reached
`input_finished`, and returned non-empty text representing every repeated source section. Ordinary
character deletions remain counted in CER and are not hidden by that no-wholesale-truncation check.
This closes the deterministic host inference/reproducibility gate only. It is not the Android
converted-CER, RSS, or thermal gate and is not a substitute for consented human speech.

## Host model bake-off after the first real-speaker failure

The poor editable draft triggered a bounded host-only bake-off rather than an unmeasured production
model swap. Both candidates are official Apache-2.0 sherpa-onnx models, used the same 50-item corpus,
two threads, Traditional-Chinese conversion, and exact CER/LCS scoring. Neither result is Android or
real-speaker acceptance.

| Model | Bytes | Converted mean CER | Code-switch CER | Long LCS | Peak RSS | Decision |
|---|---:|---:|---:|---:|---:|---|
| Current streaming Paraformer | 237,202,501 | 0.08554 | 0.19986 | 3/3 | 537,362,432 | Keep as production baseline pending device evidence |
| Streaming bilingual Zipformer `98590b7` | 198,270,793 | 0.26468 | 0.40053 | 2/3 | 430,014,464 | Reject: both CER gates and one long-input LCS gate fail |
| Offline Paraformer `def0270` | 243,446,974 | **0.06355** | **0.13242** | 3/3 | 575,193,088 | Advance only to isolated Android A/B |

The offline candidate completed 50/50 with no empty or truncated item and materially improved the
synthetic text metrics, but its maximum host RTF was `0.55089`, above the `0.5` gate, versus `0.05580`
for the retained streaming baseline. Its model load and maximum inference were also roughly 11x and
9x slower. It therefore is not installed by the production app. The isolated benchmark can measure
its Android RSS, RTF, thermal behavior, and quality without mutating the production model contract.
The Zipformer candidate is not advanced.

Exact candidate provenance:

- offline Paraformer `csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14@def027084691107096b5ebba69785756d63de6c5`:
  `model.int8.onnx` 243,371,218 bytes / SHA-256
  `f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945`; `tokens.txt`
  75,756 bytes / SHA-256 `59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6`;
- streaming Zipformer `csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20@98590b7ed6443e77b714204da2757d75e1a642f4`:
  encoder/decoder/joiner/tokens total 198,270,793 bytes. Their respective SHA-256 values are
  `8fa764187a261844f859d7143ebaa563af5d10adfece4c18a8f414c88cba2a9b`,
  `1a70c593d71e53f023f5f55b0b4cfff5055abb786ee3992e5f63dc2e273cc4fa`,
  `1ed689c5ed19dbaa725d9d191bb4822b5f4855a39e1ffd28cbc1f340d25b2ee0`, and
  `a8e0e4ec53810e433789b54a5c0134a7eaa2ffca595a6334d54c00da858841d3`.

The license/provenance sources are the pinned
[offline Paraformer model card](https://huggingface.co/csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14/blob/def027084691107096b5ebba69785756d63de6c5/README.md),
[sherpa-onnx offline model listing](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-paraformer/paraformer-models.html#csukuangfj-sherpa-onnx-paraformer-zh-2023-09-14-chinese-english),
and pinned [Zipformer repository](https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/tree/98590b7ed6443e77b714204da2757d75e1a642f4).

## Signed release package

The final audited production client was built through the Nix dev shell as a per-ABI arm64 release.
Signing values were injected from the local Keychain through `TM_ANDROID_RELEASE_*`; no
`android/key.properties` file was created. This hash includes pending-spawn cancellation,
recorder-retirement, native-failure propagation, rejected-buffer cleanup, immutable-platform-buffer
ownership, fail-closed negative recorder reads, aggregate signal diagnostics, and the installed-app
build fingerprint shown without ADB. The package version was advanced so it cannot be confused with
the earlier diagnostic artifact.

| Field | Result |
|---|---|
| APK | `clients/miku_flutter/build/app/outputs/flutter-apk/app-arm64-v8a-release.apk` |
| Version | `1.0.2+3` |
| Bytes | 52,234,268 |
| SHA-256 | `1c68fad452bd0525f21c50aeb389825e51a9893126c6c94679ea8004401c3407` |
| ABI | `arm64-v8a` only |
| Signature | APK v1 + v2 verify; one 4096-bit RSA release signer |
| Certificate SHA-256 | `503f865843464347cb2d2f90be3ab4dbf68bae690443bf8fd262e0dcc0b58ee1` |
| Manifest | `RECORD_AUDIO` visible; backup and cleartext traffic disabled |
| Content inspection | no encoder/decoder ONNX, token file, PCM, or WAV in APK |

The first non-split release was also validly signed, but sherpa's federated packages retained unused
native libraries for other ABIs. It is not used as the arm64 acceptance artifact; the verified split
above is the retained canary. The split contains only `arm64-v8a` native libraries and contains no
model weights, token file, PCM, or WAV payload.

## Local verification

The resumed slice includes deterministic tests for permission/capture state and recorder-exit
reporting, retiring-recorder start exclusion, exact size and SHA verification, redirect denial,
interrupted/corrupt/delete/idempotent and cross-instance installation, timeout/cancel worker
semantics including pending spawn, PCM conversion and decoded-source wiping, negative-read failure,
aggregate signal diagnostics, cancellation-failure retry UI, editable review, no auto-send, and
explicit current/new-session routing. Installed-build fingerprint parsing, caching, privacy, UI,
and non-blocking failure behavior also have deterministic coverage. After the final lifecycle audit
fixes, the focused Flutter voice/model tests passed, and the Kotlin capture/installer/fingerprint
tests passed as part of the 29-test `:app:testDebugUnitTest` gate. `flutter analyze` reported no
findings, and the current full Flutter suite passed 113/113, including 20 focused voice widgets and
server-authority transition regressions. The standalone Android A/B harness passed analyze and 20
tests; its host-side schema-3 verifier independently recomputes metrics and gates,
rejects missing RSS/thermal data, binds the exact local corpus and installed benchmark APK, and
requires both engines to share the same physical installation/build before emitting its comparison
JSON. The Web release compiled successfully; the standard Playwright smoke passed one test and
skipped only the separately gated evidence capture. The final diagnostics-bearing signed
release above then built and verified from the same final source. Running Gradle's broader unscoped
`testDebugUnitTest` also tries to compile third-party `mobile_scanner` tests; that separate plugin
task is incompatible with the current Jetifier because Byte Buddy uses class-file major 68. The
scoped app tests are unaffected. The final repository-wide matrix is recorded in the roadmap
closeout section after all remaining milestones are integrated.

## Physical Android progress and gates still open

On 2026-07-18 the Xiaomi `2412DPC0AG` reconnected over USB ADB as
`KVIRRSRK7TGMXWZP` on Android 15. The then-current signed arm64 split
`af8f9584910e3ef03704c382c4eba92855cc33f2e3ba29910559354188d68b51` installed in place while
preserving the prior paired state. The explicit owner action downloaded all 237,202,501 pinned model bytes;
the production installer accepted the version only after exact size, SHA-256, manifest, and private
no-backup-directory verification. The UI then reported `已安裝並驗證` / `verified`.

The app was force-stopped, Android airplane mode was enabled, and a cold start still reported the
model as installed and verified without a network path. Airplane mode was then disabled and the
device returned to its prior connected state. This closes signed install, model activation,
app-process persistence, and offline model-inspection gates. It does not by itself prove inference,
recorder cleanup, resource, thermal, or message-routing acceptance.

The first consented microphone attempt reached the production stop/inference path and exposed that
Android's MethodChannel PCM view could be immutable: cleanup raised `Cannot change unmodifiable
content` before the review sheet opened. The app sent no message. The client now clones platform
bytes into an owned writable buffer before validation/inference, wipes that clone deterministically,
and treats clearing the source view as best effort. Focused immutable-view coverage plus a combined
platform-PCM-to-transcribe-to-editable-review regression pass, an independent P0/P1 audit found no
remaining production-path issue, and an intermediate
immutable-buffer-fix signed APK installed in place without deleting the verified model or paired
session. The subsequent recording closed this specific immutable-buffer regression.

The owner then completed a second foreground recording on that intermediate immutable-buffer-fix
build. It reached the editable local review sheet without another buffer error, no destination was
selected, the send button remained disabled, and no message was sent. The draft read
`me 書提醒我明天早上七點半出門的時候借貸於`; the owner judged the result poor. Because the exact
spoken reference was not captured alongside this attempt, no CER is fabricated from it, but the
observed proper-name and terminal-phrase errors were sufficient to keep the real-speaker quality gate
open. The privacy-preserving signal diagnostics and strict same-installation A/B verifier were then
implemented. Any local replacement still must pass the same storage, RSS, latency, and offline
constraints.

The owner later tried the brief non-lexical utterance `umm` and again judged the result poor. That
probe is too short and has no Traditional-Chinese reference, so it is not counted as CER evidence or
as a frozen real-speaker corpus item.

The device reconnected over authorized USB on 2026-07-19. The final diagnostics-bearing production
APK `1.0.2+3` was installed in place without deleting the paired credential or existing app data.
The host artifact and installed `base.apk` both had SHA-256
`1c68fad452bd0525f21c50aeb389825e51a9893126c6c94679ea8004401c3407`; the APK was 52,234,268
bytes and retained the established signer certificate
`503f865843464347cb2d2f90be3ab4dbf68bae690443bf8fd262e0dcc0b58ee1`. A force-stop/cold launch
completed in 1.068 seconds and returned to the already paired connected chat.

With explicit consent and the exact reference `幫我記得今天晚上九點要倒垃圾。`, one foreground
recording reached the editable review and produced the unedited draft
`幫我記得今天晚上九點要倒的是`. After the frozen punctuation normalization this is two
substitutions over 14 reference characters (`0.142857`). The review reported 4.98 seconds,
RMS `-23.4 dBFS`, peak `-5.1 dBFS`, 96.4% active frames, 0% clipped samples, 0.5% near-zero
samples, 80 ms leading silence, and 0 ms trailing silence. It displayed the exact installed-build
fingerprint, no destination was selected, Send remained disabled, and no message was sent. This
closes final-package install/fingerprint and a single healthy-signal review canary. At that point it
did not close the ten-item consented corpus, lifecycle/cleanup, or exact current/new send matrix; the owner also
judged the local model's language habits insufficiently Taiwan-specific.

## Physical Android synthetic A/B

On 2026-07-19 the isolated, debug-only, permission-free benchmark ran both pinned engines on the
same physical Android installation, Android build, benchmark APK, and 50-item synthetic corpus. The
host verifier independently recomputed both reports and emitted the retained
[`same-device A/B report`](2026-07-19-p6-6-android-asr-ab.json). Both engines completed all 50 cases,
all three 59–60 second cases, produced no empty output, stayed below the 1 GiB RSS and 0.5 RTF
limits, and observed thermal status `none` throughout.

| Metric | Streaming production | Offline candidate |
|---|---:|---:|
| Converted mean CER | 0.08709 | **0.06540** |
| Code-switch converted mean CER | 0.18855 | **0.12112** |
| Maximum RTF | 0.11879 | **0.08577** |
| Maximum inference | **4,242 ms** | 4,417 ms |
| Model load | **1,539 ms** | 1,637 ms |
| Peak RSS | **712,772 KiB** | 864,112 KiB |
| Maximum thermal status | none | none |

The candidate improves the synthetic text and RTF metrics but consumes 151,340 KiB more peak RSS.
This closes the frozen same-device synthetic A/B gate; it does not measure a human speaker or the
production capture/review lifecycle. The production app therefore retains the smaller streaming
baseline until the consented real-speaker evidence can justify a model-contract migration. The
owner-selectable home TEA-ASR route remains the Taiwan-Mandarin alternative in the meantime.

## Physical non-recording lifecycle and model recovery

The signed `1.0.3+4` release then passed the physical gates that do not require another person's
speech. Revoking `RECORD_AUDIO` and pressing the visible microphone action produced the Android
permission sheet; choosing **不允許** left the permission false, set AppOps to `ignore`, started no
new AudioService recording, and showed `未授予麥克風權限。` in the app. The prior granted +
`USER_SET` permission state was restored after the check.

The release model manager started at `已安裝並驗證`, deleted the exact 237,202,501-byte model, and
immediately reported `尚未安裝`. Its explicit install disclosure named the fixed commit, 226 MiB
download, Apache-2.0 license, Android no-backup private storage, offline recognition, and mandatory
review. A complete reinstall returned to `verified`; force-stop/cold-start then retained both the
verified model and paired connection. A second install was force-stopped 0.5 seconds after download
confirmation. The next cold start retained pairing but correctly reported `尚未安裝`, never accepted
the partial staging data, and a final complete reinstall again returned to `verified`.

The release is intentionally non-debuggable, so ADB `run-as` cannot be used to weaken or mutate its
private model files. The permanent debug build now uses the isolated
`org.mozufu.tempestmiku.uitest` application id. On that same phone, the current `1.0.3-uitest` code
was given a correctly named model directory containing an unexpected file. The real model manager
reported `檔案毀損，已停用` / `voice model directory had unexpected files`, never `verified`.
Choosing Install removed the corrupt version, downloaded and atomically activated the pinned model,
and produced the three exact production hashes. The `.uitest` model was then deleted and its model
tree contained zero files; the signed production app still reported `verified` afterward.

Installed release package flags omit `ALLOW_BACKUP`, the APK manifest has backup disabled, and the
model/capture roots are created below `noBackupFilesDir`. Scans of the app's external data, media,
and OBB locations found zero files before and after delete/reinstall. This closes permission denial,
delete, interrupted-install cleanup, corrupt-disable/recovery, restart persistence, and backup/
external-residue gates without claiming that release-private storage was readable through ADB.

## Optional self-hosted Taiwan-Mandarin engine

On 2026-07-19 the owner requested a controllable home-hosted alternative rather than treating a
different cloud provider as the answer. The current homelab service is
[`JacobLinCool/TEA-ASR-1.1-mini`](https://huggingface.co/JacobLinCool/TEA-ASR-1.1-mini), not the
older Parakeet deployment. Its health endpoint responded successfully before integration. TEA-ASR
is a 780M Taiwan-Mandarin adaptation of Qwen3-ASR and its model card claims native Traditional
Chinese, Taiwan vocabulary, and code-switching; those are upstream claims rather than independent
TempestMiku acceptance evidence.

TempestMiku therefore adds a separate `self_hosted` engine while preserving `local` as the default.
The remote option is absent unless the operator supplies all of
`TM_SELF_HOSTED_ASR_ENDPOINT`, `TM_SELF_HOSTED_ASR_LABEL`, and
`TM_SELF_HOSTED_ASR_MODEL_ID`; partial configuration fails startup. The Android client receives
only bounded public engine metadata and can never submit or discover the upstream URL. Selecting
the engine requires an explicit drawer disclosure. Each foreground stop uploads exactly one bounded
16 kHz mono PCM16 buffer through the authenticated TempestMiku server; the server constructs WAV in
memory, disables proxy discovery and redirects, bounds the request/response and 45-second operation,
and retains neither audio nor transcript. Remote failure produces no review and never calls the
local recognizer. Successful text still enters editable review with remote provenance and cannot be
sent without a separate current/new-session confirmation.

Five existing non-human synthetic corpus items were sent directly to the healthy homelab service
before the broker canary. Taiwan-local item 011, conversational correction item 029, and the GitHub
code-switch item 035 matched their normalized references. Item 040 misrecognized the proper name
`NVIDIA Parakeet`, and the long engineering item 049 preserved the overall repeated structure but
made several English/code-switch and Traditional-character errors. This is sufficient to justify a
visible owner-selectable trial, not to claim that TEA-ASR uniformly beats the local model. The
signed-app capture path still requires its own integrated recording canary, and any future engine
replacement must preserve the same no-fallback, no-retention, editable-review boundary.

The broker itself then passed an isolated live end-to-end smoke. A loopback `tm-server` loaded the
fixed homelab config, issued a one-time Android-style device credential, advertised only the public
`self_hosted` label/model metadata, and accepted 113,580 bytes of PCM16 derived from synthetic item
011. The server constructed the WAV/multipart request and returned
`從臺北車站搭捷運到市政府要多久` with engine id `self_hosted` and the configured TEA-ASR model id.
No endpoint appeared in the catalog or response. The isolated server was stopped and its temporary
PCM and one-time cookie material were deleted immediately after the run. The same fixed configuration
was subsequently deployed on the paired lumo production server: lumo advertises only the label and
model id, reaches homolab over the owner's tailnet, and reports a healthy upstream. This proves the
real authenticated server-to-homelab path, but not the still-open signed Android recording and
cancellation canary.

### Signed client containing the self-hosted selector

The new client path, native timeout/abort behavior, and server-authority epoch fences were built as
another Keychain-signed arm64 split. Re-pairing or disconnecting must cancel active recording or
remote transcription before credentials change; a cleanup failure blocks that authority change.
Every attempted server transition immediately revokes the prior remote-engine consent and restores
local selection, while stale catalogs, dialogs, requests, and transcripts are ignored. Focused
Flutter voice widgets passed 20/20, the full Flutter suite passed 113/113, and analyze reported no
issues.

| Field | Result |
|---|---|
| Version | `1.0.3+4` |
| Bytes | 52,300,150 |
| SHA-256 | `b9cede23fc918c2d1a76c3ce3ef5f72a3a1680716ef0c6b6b58c038997f56079` |
| Certificate SHA-256 | `503f865843464347cb2d2f90be3ab4dbf68bae690443bf8fd262e0dcc0b58ee1` |
| Content inspection | one `arm64-v8a` split; v1/v2 signature; no model, token file, PCM, or WAV |
| Physical state | installed in place; device `base.apk` matched the host SHA-256, pairing and app data retained |
| Signed-app selector | PASS; catalog enabled `Home TEA-ASR (Taiwan Mandarin)` / `JacobLinCool/TEA-ASR-1.1-mini`, disclosure confirmed, explicit selection made without audio upload |

The install, catalog, disclosure, and explicit-selection portion of the signed-app canary now passes.

The owner subsequently gave explicit consent and completed the signed-app capture path on that exact
installed build. A 5.02-second foreground capture had RMS `-18.3 dBFS`, peak `-4.5 dBFS`, 96.8%
active speech, 0.00% clipping, and no aggregate quality warning. The app sent the bounded buffer
through the paired production server to the fixed home service, returned the exact separately stated
reference after punctuation normalization, and opened editable review with `self_hosted` provenance.
Neither destination was selected, the send action remained disabled, and the review was cancelled;
no message was sent and the UI stated that the app-side audio had been wiped. An earlier deliberately
cancelled remote capture stopped in AudioService, returned to an empty composer, created no review,
and never invoked the local recognizer. This closes the signed Android self-hosted recording,
editable-review, explicit-cancel, no-fallback, and no-automatic-send canaries. It was not substituted
for the separate production-local real-speaker score below.

### Signed-client background and process-death cleanup

The owner had already consented to the production lifecycle matrix. On the exact signed `1.0.3+4`
installation, a foreground local capture started at `14:38:08` and was sent to the launcher while
still recording. `MainActivity.onPause` stopped AudioRecord at `14:38:19`; AudioService recorded a
matching `rec stop`, no package service remained, and returning to the hot task showed an empty
composer with no review or enabled send action.

A second capture started at `14:38:45` and `am force-stop org.mozufu.tempestmiku` killed the real
production PID at `14:38:56`. Android first silenced and then released the package's AudioRecord by
`14:38:56.300`; no package PID or service remained. A subsequent explicit component launch was a
real cold start, retained pairing, and again showed no draft, review, or enabled send action. A scan
of the package's external data/media roots found no file. The release package correctly rejects
`run-as`, so ADB cannot directly inspect the app-private `noBackupFilesDir`; startup orphan purging
is therefore supported by the already passing exact Kotlin implementation tests plus the observed
cold-start no-replay/no-service behavior rather than a false claim of direct private-file access.
No capture was transcribed or sent during either lifecycle canary.

### Signed-client unavailable-upstream failure

The production `tea-asr-1-1-mini.service` first returned `{"status":"ok"}`. It was then stopped
through systemd at `14:40:52`; both `systemctl is-active` and a direct tailnet health request proved
the unit inactive and the fixed port connection refused. The signed client explicitly selected the
home engine after showing its remote-audio disclosure, started a capture at `14:41:40`, and stopped
at `14:41:52`. The app returned to an empty composer: there was no remote review, no local-provenance
review, no enabled send action, and the selected durable session remained at exactly zero messages.
This is the physical failure counterpart to the deterministic client/server no-fallback tests; no
claim is inferred from the healthy-path canary alone.

The homolab unit was immediately started again. Its systemd state returned active first, and the
canary continued waiting until the actual health endpoint again returned `{"status":"ok"}`. The
Android selector was then explicitly restored to `本機 · 音訊留在裝置上`. Thus the failure was
bounded and the production home service was not left degraded.

### Exact current/new-session sends

The owner reviewed two separate production-local drafts and explicitly chose their destinations on
the signed `1.0.3+4` package. The current-session draft entered existing session
`761985ba-f64f-44a9-a37b-02222ca53d5a` once; the session grew by the expected user turn plus reply,
and PostgreSQL contained the exact user content once. The new-session draft entered distinct session
`1eefc71e-7ae1-4e0c-acdb-77a80cae4db5` once; PostgreSQL again contained the exact user content once,
and the client selected the new session. Neither destination was inferred, no recording auto-sent,
and the two raw draft texts are not retained in this durable evidence.

### Consented production-local real-speaker quality

The owner explicitly consented to ten exact-reference recordings on the same installed production
package. The local streaming model produced editable drafts only; no corpus audio was uploaded or
retained, and none of these quality items was automatically sent. The frozen evaluator passed all
gates:

- 10/10 required items and coverage tags;
- converted mean CER `0.13292244424811311` (limit `0.20`);
- converted p90 CER `0.2413793103448276` (limit `0.35`);
- code-switch converted mean CER `0.28197997775305894` (limit `0.30`);
- zero empty, truncated, or aggregate signal-warning items.

The code-switch result passes narrowly and does not justify claiming universally natural Taiwan
Mandarin transcription. The privacy-stripped retained
[`real-speaker evaluation`](2026-07-19-p6-6-real-speaker-eval.json) binds the aggregate to the exact
APK, certificate, model, conversion, and canonical raw-input digest. Before raw input deletion, the
report was independently recomputed and matched after excluding only its evaluation timestamp. The
owner-local raw references, hypotheses, random capture ids, and device fingerprint were then removed;
the retained report contains per-item CER and lengths but no reconstructable transcript or audio.

These physical results close P6.6 and full P6. Any future recording remains separately opt-in and
must not weaken editable review, explicit destination selection, no-fallback, no-retention, or
no-automatic-send boundaries.
