#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CACHE="$ROOT/.asr-benchmark"
PACKAGE="org.mozufu.tempestmiku.asrbenchmark"
CORPUS_DIR="$CACHE/corpus"
CORPUS_MANIFEST="$CACHE/corpus-manifest.json"
CORPUS_SOURCE="$ROOT/corpus/zh-tw-synthetic-v1.tsv"
CORPUS_SOURCE_SHA256="424fc2e9baf3f40240f7e23ffc7aacd1f00f71a4780074b82c153560873fd621"
APK="$ROOT/build/app/outputs/flutter-apk/app-debug.apk"
REPORT_SCHEMA=3

ADB_BIN="${ADB_BIN:-}"
if [[ -z "$ADB_BIN" ]]; then
  if command -v adb >/dev/null 2>&1; then
    ADB_BIN="$(command -v adb)"
  elif [[ -n "${ANDROID_SDK_ROOT:-}" && -x "$ANDROID_SDK_ROOT/platform-tools/adb" ]]; then
    ADB_BIN="$ANDROID_SDK_ROOT/platform-tools/adb"
  elif [[ -n "${ANDROID_HOME:-}" && -x "$ANDROID_HOME/platform-tools/adb" ]]; then
    ADB_BIN="$ANDROID_HOME/platform-tools/adb"
  fi
fi

COMMAND="${1:-}"
ENGINE="${2:-${ASR_BENCHMARK_ENGINE:-streaming-production}}"

MODEL_CONTRACT=""
MODEL_COMMIT=""
MODEL_ID=""
MODEL_REPOSITORY=""
MODEL_BASE_URL=""
MODEL_DIR=""
MODEL_TOTAL_BYTES=0
MODEL_ATTRIBUTION=""
MODEL_ROLES=()
MODEL_NAMES=()
MODEL_FILE_BYTES=()
MODEL_SHA256=()

usage() {
  cat <<'EOF'
usage: benchmark.sh {prepare|verify-corpus|build|verify-apk|install|push|run|result|verify-result|all} [engine]
       benchmark.sh verify-pair PRODUCTION_REPORT.json CANDIDATE_REPORT.json

engines:
  streaming-production          exact production streaming Paraformer (default)
  offline-paraformer-candidate  pinned whole-audio offline Paraformer candidate

ASR_BENCHMARK_ENGINE may be used instead of the second argument.
EOF
}

select_engine() {
  case "$ENGINE" in
    streaming-production)
      MODEL_CONTRACT="tempestmiku.streaming-paraformer.v1"
      MODEL_COMMIT="2a7f71bb58885c1b522ed4e683abd397355d9fc4"
      MODEL_ID="csukuangfj/sherpa-onnx-streaming-paraformer-zh@$MODEL_COMMIT"
      MODEL_REPOSITORY="https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-zh"
      MODEL_DIR="$CACHE/streaming-paraformer-zh-$MODEL_COMMIT"
      MODEL_TOTAL_BYTES=237202501
      MODEL_ATTRIBUTION="sherpa-onnx streaming Paraformer Chinese model by csukuangfj"
      MODEL_ROLES=(encoder decoder tokens)
      MODEL_NAMES=(encoder.int8.onnx decoder.int8.onnx tokens.txt)
      MODEL_FILE_BYTES=(165462184 71664561 75756)
      MODEL_SHA256=(
        81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a
        f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f
        59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6
      )
      ;;
    offline-paraformer-candidate)
      MODEL_CONTRACT="tempestmiku.offline-paraformer-candidate.v1"
      MODEL_COMMIT="def027084691107096b5ebba69785756d63de6c5"
      MODEL_ID="csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14@$MODEL_COMMIT"
      MODEL_REPOSITORY="https://huggingface.co/csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14"
      MODEL_DIR="$CACHE/offline-paraformer-zh-2023-09-14-$MODEL_COMMIT"
      MODEL_TOTAL_BYTES=243446974
      MODEL_ATTRIBUTION="sherpa-onnx offline Paraformer Chinese model by csukuangfj"
      MODEL_ROLES=(model tokens)
      MODEL_NAMES=(model.int8.onnx tokens.txt)
      MODEL_FILE_BYTES=(243371218 75756)
      MODEL_SHA256=(
        f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945
        59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6
      )
      ;;
    *)
      echo "unknown benchmark engine: $ENGINE" >&2
      usage >&2
      exit 2
      ;;
  esac
  MODEL_BASE_URL="$MODEL_REPOSITORY/resolve/$MODEL_COMMIT"
  MANIFEST_DIR="$CACHE/manifests"
  MANIFEST="$MANIFEST_DIR/$ENGINE.json"
}

adb_cmd() {
  [[ -n "$ADB_BIN" && -x "$ADB_BIN" ]] || {
    echo "adb is unavailable; set ADB_BIN to the platform-tools adb executable" >&2
    return 127
  }
  if [[ -n "${ADB_SERIAL:-}" ]]; then
    "$ADB_BIN" -s "$ADB_SERIAL" "$@"
  else
    "$ADB_BIN" "$@"
  fi
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

file_bytes() {
  if stat -f %z "$1" >/dev/null 2>&1; then
    stat -f %z "$1"
  else
    stat -c %s "$1"
  fi
}

verify_file() {
  local path="$1"
  local expected_bytes="$2"
  local expected_sha="$3"
  local actual_bytes actual_sha
  [[ -f "$path" ]] || return 1
  actual_bytes="$(file_bytes "$path")"
  actual_sha="$(sha256_file "$path")"
  [[ "$actual_bytes" == "$expected_bytes" ]] || {
    echo "file size mismatch for $path: got $actual_bytes, expected $expected_bytes" >&2
    return 1
  }
  [[ "$actual_sha" == "$expected_sha" ]] || {
    echo "file SHA-256 mismatch for $path: got $actual_sha" >&2
    return 1
  }
}

resolve_aapt2() {
  if [[ -n "${AAPT2:-}" && -x "$AAPT2" ]]; then
    printf '%s\n' "$AAPT2"
    return
  fi
  if command -v aapt2 >/dev/null 2>&1; then
    command -v aapt2
    return
  fi
  local sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  local candidate
  candidate="$(
    find "$sdk_root/build-tools" -type f -name aapt2 2>/dev/null |
      sort -V |
      tail -n 1 || true
  )"
  [[ -n "$candidate" && -x "$candidate" ]] || return 1
  printf '%s\n' "$candidate"
}

verify_corpus() {
  [[ -f "$CORPUS_SOURCE" ]] || {
    echo "missing canonical corpus source: $CORPUS_SOURCE" >&2
    return 1
  }
  [[ "$(sha256_file "$CORPUS_SOURCE")" == "$CORPUS_SOURCE_SHA256" ]] || {
    echo "canonical corpus source digest drifted" >&2
    return 1
  }
  [[ -f "$CORPUS_MANIFEST" ]] || {
    echo "missing $CORPUS_MANIFEST; regenerate the corpus" >&2
    return 1
  }
  jq -e \
    --arg source_sha "$CORPUS_SOURCE_SHA256" \
    '
      .schema == 3 and
      .id == "zh-tw-meijia-synthetic-v1" and
      .kind == "synthetic_tts" and
      .source == "corpus/zh-tw-synthetic-v1.tsv" and
      .sourceSha256 == $source_sha and
      .items == 50 and .longItems == 3 and
      (.cases | length) == 50 and
      ([.cases[] | select(.long == true)] | length) == 3 and
      (all(.cases[0:47][]; .long == false)) and
      (all(.cases[47:50][]; .long == true)) and
      (all(.cases[];
        (.file | test("^zh-tw-synth-[0-9]{3}-[a-z-]+\\.wav$")) and
        (.reference | test("^zh-tw-synth-[0-9]{3}-[a-z-]+\\.txt$")) and
        (.audioBytes | type == "number") and .audioBytes > 0 and
        (.referenceBytes | type == "number") and .referenceBytes > 0 and
        (.audioSha256 | test("^[0-9a-f]{64}$")) and
        (.referenceSha256 | test("^[0-9a-f]{64}$"))))
    ' "$CORPUS_MANIFEST" >/dev/null || {
    echo "corpus manifest is stale or malformed; regenerate it with generate-zh-tw-corpus.sh" >&2
    return 1
  }

  local file reference audio_bytes audio_sha reference_bytes reference_sha
  while IFS=$'\t' read -r \
    file reference audio_bytes audio_sha reference_bytes reference_sha; do
    verify_file "$CORPUS_DIR/$file" "$audio_bytes" "$audio_sha"
    verify_file "$CORPUS_DIR/$reference" "$reference_bytes" "$reference_sha"
  done < <(
    jq -r '.cases[] | [
      .file,
      .reference,
      (.audioBytes | tostring),
      .audioSha256,
      (.referenceBytes | tostring),
      .referenceSha256
    ] | @tsv' "$CORPUS_MANIFEST"
  )
}

download_file() {
  local name="$1"
  local path="$2"
  local expected_bytes="$3"
  local expected_sha="$4"
  local partial="$path.partial"
  if verify_file "$path" "$expected_bytes" "$expected_sha"; then
    return
  fi
  echo "downloading pinned $ENGINE model file $name"
  curl \
    --fail \
    --location \
    --max-redirs 1 \
    --proto '=https' \
    --proto-redir '=https' \
    --retry 3 \
    --continue-at - \
    --output "$partial" \
    "$MODEL_BASE_URL/$name"
  verify_file "$partial" "$expected_bytes" "$expected_sha"
  mv "$partial" "$path"
}

write_manifest() {
  mkdir -p "$MANIFEST_DIR"
  if [[ "$ENGINE" == "streaming-production" ]]; then
    jq -n \
      --arg engine "$ENGINE" \
      --arg contract "$MODEL_CONTRACT" \
      --arg model_id "$MODEL_ID" \
      --arg repository "$MODEL_REPOSITORY" \
      --arg commit "$MODEL_COMMIT" \
      --arg source_revision_url "$MODEL_REPOSITORY/tree/$MODEL_COMMIT" \
      --arg attribution "$MODEL_ATTRIBUTION" \
      --arg encoder_sha "${MODEL_SHA256[0]}" \
      --arg decoder_sha "${MODEL_SHA256[1]}" \
      --arg tokens_sha "${MODEL_SHA256[2]}" \
      --argjson encoder_bytes "${MODEL_FILE_BYTES[0]}" \
      --argjson decoder_bytes "${MODEL_FILE_BYTES[1]}" \
      --argjson tokens_bytes "${MODEL_FILE_BYTES[2]}" \
      --argjson total_bytes "$MODEL_TOTAL_BYTES" \
      '{
        schema: 2,
        engine: $engine,
        contract: $contract,
        modelId: $model_id,
        repository: $repository,
        commit: $commit,
        sourceRevisionUrl: $source_revision_url,
        licenseName: "Apache-2.0",
        licenseUrl: "https://www.apache.org/licenses/LICENSE-2.0",
        attribution: $attribution,
        totalBytes: $total_bytes,
        runtime: {package: "sherpa_onnx", version: "1.13.4", provider: "cpu", threads: 2},
        inference: {
          mode: "streaming",
          sampleRate: 16000,
          featureDimension: 80,
          chunkSamples: 1600,
          tailPaddingSamples: 16000,
          maxDecodeSteps: 20000,
          decodingMethod: "greedy_search",
          endpointDetection: false
        },
        conversion: {
          platform: "android_icu",
          transliterator: "Simplified-Traditional",
          minimumAndroidSdk: 29
        },
        files: [
          {role: "encoder", path: "encoder.int8.onnx", bytes: $encoder_bytes, sha256: $encoder_sha},
          {role: "decoder", path: "decoder.int8.onnx", bytes: $decoder_bytes, sha256: $decoder_sha},
          {role: "tokens", path: "tokens.txt", bytes: $tokens_bytes, sha256: $tokens_sha}
        ]
      }' >"$MANIFEST"
  else
    jq -n \
      --arg engine "$ENGINE" \
      --arg contract "$MODEL_CONTRACT" \
      --arg model_id "$MODEL_ID" \
      --arg repository "$MODEL_REPOSITORY" \
      --arg commit "$MODEL_COMMIT" \
      --arg source_revision_url "$MODEL_REPOSITORY/tree/$MODEL_COMMIT" \
      --arg attribution "$MODEL_ATTRIBUTION" \
      --arg model_sha "${MODEL_SHA256[0]}" \
      --arg tokens_sha "${MODEL_SHA256[1]}" \
      --argjson model_bytes "${MODEL_FILE_BYTES[0]}" \
      --argjson tokens_bytes "${MODEL_FILE_BYTES[1]}" \
      --argjson total_bytes "$MODEL_TOTAL_BYTES" \
      '{
        schema: 2,
        engine: $engine,
        contract: $contract,
        modelId: $model_id,
        repository: $repository,
        commit: $commit,
        sourceRevisionUrl: $source_revision_url,
        licenseName: "Apache-2.0",
        licenseUrl: "https://www.apache.org/licenses/LICENSE-2.0",
        attribution: $attribution,
        totalBytes: $total_bytes,
        runtime: {package: "sherpa_onnx", version: "1.13.4", provider: "cpu", threads: 2},
        inference: {
          mode: "offline_whole_audio",
          sampleRate: 16000,
          featureDimension: 80,
          inputMode: "whole_audio",
          decodeTrigger: "after_input_complete",
          decodingMethod: "greedy_search"
        },
        conversion: {
          platform: "android_icu",
          transliterator: "Simplified-Traditional",
          minimumAndroidSdk: 29
        },
        files: [
          {role: "model", path: "model.int8.onnx", bytes: $model_bytes, sha256: $model_sha},
          {role: "tokens", path: "tokens.txt", bytes: $tokens_bytes, sha256: $tokens_sha}
        ]
      }' >"$MANIFEST"
  fi
}

prepare() {
  mkdir -p "$CACHE" "$CORPUS_DIR" "$MODEL_DIR"
  local index name
  for index in "${!MODEL_NAMES[@]}"; do
    name="${MODEL_NAMES[$index]}"
    download_file \
      "$name" \
      "$MODEL_DIR/$name" \
      "${MODEL_FILE_BYTES[$index]}" \
      "${MODEL_SHA256[$index]}"
  done
  write_manifest
  echo "prepared $ENGINE under $MODEL_DIR"
  if [[ ! -f "$CORPUS_MANIFEST" ]]; then
    echo "corpus is not generated; run tools/android_asr_benchmark/generate-zh-tw-corpus.sh" >&2
  fi
}

verify_apk() {
  [[ -f "$APK" ]] || { echo "missing APK; run build first" >&2; return 1; }
  local aapt2
  aapt2="$(resolve_aapt2)" || {
    echo "aapt2 is required to verify the harness APK contract" >&2
    return 1
  }
  local forbidden
  forbidden="$(
    unzip -Z1 "$APK" |
      grep -Ei '(^|/)(model-manifest\.json|tokens\.txt|[^/]+\.(onnx|wav|pcm|flac|mp3|m4a|aac))$' || true
  )"
  if [[ -n "$forbidden" ]]; then
    echo "APK bundled forbidden model/audio material:" >&2
    echo "$forbidden" >&2
    return 1
  fi
  local badging
  badging="$("$aapt2" dump badging "$APK")"
  [[ "$(sed -n "s/^package: name='\([^']*\)'.*/\1/p" <<<"$badging")" == "$PACKAGE" ]] || {
    echo "APK package id does not match the isolated harness" >&2
    return 1
  }
  grep -q '^application-debuggable$' <<<"$badging" || {
    echo "ASR harness APK must remain debug-only" >&2
    return 1
  }
  local permissions manifest_xml abis
  permissions="$(sed -n "s/^uses-permission: name='\([^']*\)'.*/\1/p" <<<"$badging")"
  [[ -z "$permissions" ]] || {
    echo "ASR harness APK unexpectedly requests permissions:" >&2
    echo "$permissions" >&2
    return 1
  }
  manifest_xml="$("$aapt2" dump xmltree "$APK" --file AndroidManifest.xml)"
  grep -Eq 'allowBackup\([^)]*\)=false' <<<"$manifest_xml"
  grep -Eq 'usesCleartextTraffic\([^)]*\)=false' <<<"$manifest_xml"
  abis="$(
    unzip -Z1 "$APK" |
      awk -F/ '$1 == "lib" && NF == 3 {print $2}' |
      sort -u
  )"
  [[ "$abis" == "arm64-v8a" ]] || {
    echo "ASR harness APK must contain only arm64-v8a native libraries; got: $abis" >&2
    return 1
  }
  echo "verified isolated debug-only arm64 APK with no permissions or model/audio payload"
}

build() {
  cd "$ROOT"
  flutter pub get
  flutter build apk --debug --target-platform android-arm64
  verify_apk
}

install_apk() {
  [[ -f "$APK" ]] || { echo "missing APK; run build first" >&2; return 1; }
  adb_cmd install -r "$APK"
}

push_private() {
  local source="$1"
  local destination="$2"
  local temporary="/data/local/tmp/tm-asr-${destination//\//-}"
  adb_cmd push "$source" "$temporary"
  adb_cmd shell run-as "$PACKAGE" cp "$temporary" "files/asr-benchmark/$destination"
  adb_cmd shell rm "$temporary"
}

push_data() {
  prepare
  verify_corpus
  adb_cmd shell run-as "$PACKAGE" rm -rf files/asr-benchmark
  adb_cmd shell run-as "$PACKAGE" mkdir -p files/asr-benchmark/corpus
  local index name
  for index in "${!MODEL_NAMES[@]}"; do
    name="${MODEL_NAMES[$index]}"
    push_private "$MODEL_DIR/$name" "$name"
  done
  push_private "$MANIFEST" model-manifest.json
  push_private "$CORPUS_MANIFEST" corpus-manifest.json

  shopt -s nullglob
  local wav reference
  for wav in "$CORPUS_DIR"/*.wav; do
    name="$(basename "$wav")"
    push_private "$wav" "corpus/$name"
    reference="${wav%.wav}.txt"
    if [[ -f "$reference" ]]; then
      push_private "$reference" "corpus/${name%.wav}.txt"
    fi
  done
  shopt -u nullglob
}

run_app() {
  adb_cmd shell am force-stop "$PACKAGE"
  adb_cmd shell monkey -p "$PACKAGE" -c android.intent.category.LAUNCHER 1
}

print_result() {
  local report
  report="$(adb_cmd shell run-as "$PACKAGE" cat files/asr-benchmark/last-result.json)"
  jq -e \
    --arg engine "$ENGINE" \
    --argjson schema "$REPORT_SCHEMA" \
    '.schema == $schema and .engine == $engine' \
    <<<"$report" >/dev/null || {
    echo "device report is stale or belongs to a different engine" >&2
    return 1
  }
  printf '%s\n' "$report"
}

verify_result() {
  local report
  report="$(print_result)"
  verify_corpus
  [[ -f "$APK" ]] || {
    echo "missing benchmark APK for digest binding: $APK" >&2
    return 1
  }
  printf '%s\n' "$report" | (
    cd "$ROOT"
    dart run bin/verify_report.dart "$ENGINE" "$CORPUS_MANIFEST" "$APK"
  )
}

verify_pair() {
  local production_report="$1"
  local candidate_report="$2"
  [[ -f "$production_report" ]] || {
    echo "missing production report: $production_report" >&2
    return 1
  }
  [[ -f "$candidate_report" ]] || {
    echo "missing candidate report: $candidate_report" >&2
    return 1
  }
  production_report="$(cd -- "$(dirname -- "$production_report")" && pwd)/$(basename -- "$production_report")"
  candidate_report="$(cd -- "$(dirname -- "$candidate_report")" && pwd)/$(basename -- "$candidate_report")"
  verify_corpus
  [[ -f "$APK" ]] || {
    echo "missing benchmark APK for digest binding: $APK" >&2
    return 1
  }
  (
    cd "$ROOT"
    dart run bin/verify_pair.dart \
      "$production_report" \
      "$candidate_report" \
      "$CORPUS_MANIFEST" \
      "$APK"
  )
}

if [[ "$COMMAND" == "verify-pair" ]]; then
  if [[ "$#" -ne 3 ]]; then
    usage >&2
    exit 2
  fi
  verify_pair "$2" "$3"
  exit
fi

select_engine
case "$COMMAND" in
  prepare) prepare ;;
  verify-corpus) verify_corpus ;;
  build) build ;;
  verify-apk) verify_apk ;;
  install) install_apk ;;
  push) push_data ;;
  run) run_app ;;
  result) print_result ;;
  verify-result) verify_result ;;
  all)
    verify_corpus
    build
    install_apk
    push_data
    run_app
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
