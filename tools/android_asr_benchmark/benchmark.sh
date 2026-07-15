#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CACHE="$ROOT/.asr-benchmark"
PACKAGE="org.mozufu.tempestmiku.asrbenchmark"
MODEL_NAME="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
ARCHIVE="$CACHE/$MODEL_NAME.tar.bz2"
ARCHIVE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$MODEL_NAME.tar.bz2"
ARCHIVE_BYTES=163002883
ARCHIVE_SHA256="7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e"
MODEL_DIR="$CACHE/$MODEL_NAME"
MODEL_FILE="$MODEL_DIR/model.int8.onnx"
TOKENS_FILE="$MODEL_DIR/tokens.txt"
CORPUS_DIR="$CACHE/corpus"
MANIFEST="$CACHE/model-manifest.json"
CORPUS_MANIFEST="$CACHE/corpus-manifest.json"
APK="$ROOT/build/app/outputs/flutter-apk/app-debug.apk"

usage() {
  echo "usage: $0 {prepare|build|install|push|run|result|all}"
}

adb_cmd() {
  if [[ -n "${ADB_SERIAL:-}" ]]; then
    command adb -s "$ADB_SERIAL" "$@"
  else
    command adb "$@"
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

verify_archive() {
  local actual_bytes actual_sha
  actual_bytes="$(file_bytes "$ARCHIVE")"
  actual_sha="$(sha256_file "$ARCHIVE")"
  [[ "$actual_bytes" == "$ARCHIVE_BYTES" ]] || {
    echo "archive size mismatch: got $actual_bytes, expected $ARCHIVE_BYTES" >&2
    return 1
  }
  [[ "$actual_sha" == "$ARCHIVE_SHA256" ]] || {
    echo "archive SHA-256 mismatch: got $actual_sha" >&2
    return 1
  }
}

prepare() {
  mkdir -p "$CACHE" "$CORPUS_DIR"
  if [[ ! -f "$ARCHIVE" ]] || ! verify_archive; then
    echo "downloading pinned SenseVoice archive (a partial file will resume)"
    curl -fL --retry 3 -C - -o "$ARCHIVE" "$ARCHIVE_URL"
    verify_archive
  fi
  if [[ ! -f "$MODEL_FILE" ]] || [[ ! -f "$TOKENS_FILE" ]]; then
    tar -xjf "$ARCHIVE" -C "$CACHE"
  fi
  [[ -f "$MODEL_FILE" ]] || { echo "missing $MODEL_FILE" >&2; return 1; }
  [[ -f "$TOKENS_FILE" ]] || { echo "missing $TOKENS_FILE" >&2; return 1; }

  if [[ ! -f "$CORPUS_DIR/zh.wav" ]]; then
    cp "$MODEL_DIR/test_wavs/zh.wav" "$CORPUS_DIR/zh.wav"
  fi
  if [[ ! -f "$CORPUS_DIR/en.wav" ]]; then
    cp "$MODEL_DIR/test_wavs/en.wav" "$CORPUS_DIR/en.wav"
  fi
  printf '%s\n' '开放时间早上9点至下午5点。' >"$CORPUS_DIR/zh.txt"
  printf '%s\n' 'The tribal chieftain called for the boy and presented him with 50 pieces of code.' >"$CORPUS_DIR/en.txt"

  if [[ ! -f "$CORPUS_MANIFEST" ]]; then
    jq -n '{
      schema: 1,
      id: "sensevoice-upstream-sanity-v1",
      kind: "upstream_fixtures",
      locale: "mixed",
      items: 2,
      longItems: 0,
      limitations: [
        "Upstream fixtures establish runtime sanity only.",
        "They are not a Taiwan Mandarin accuracy corpus."
      ]
    }' >"$CORPUS_MANIFEST"
  fi

  jq -n \
    --arg model_sha "$(sha256_file "$MODEL_FILE")" \
    --arg tokens_sha "$(sha256_file "$TOKENS_FILE")" \
    '{
      schema: 1,
      modelId: "sensevoice-small-int8-2024-07-17",
      modelFile: "model.int8.onnx",
      modelSha256: $model_sha,
      tokensFile: "tokens.txt",
      tokensSha256: $tokens_sha,
      licenseName: "FunASR Model Open Source License Agreement 1.1",
      licenseUrl: "https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE",
      attribution: "SenseVoiceSmall by FunAudioLLM/FunASR",
      language: "auto",
      useInverseTextNormalization: true,
      threads: 4
    }' >"$MANIFEST"
  echo "prepared model and corpus under $CACHE"
}

build() {
  cd "$ROOT"
  flutter pub get
  flutter build apk --debug --target-platform android-arm64
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
  adb_cmd shell run-as "$PACKAGE" rm -rf files/asr-benchmark
  adb_cmd shell run-as "$PACKAGE" mkdir -p files/asr-benchmark/corpus
  push_private "$MODEL_FILE" model.int8.onnx
  push_private "$TOKENS_FILE" tokens.txt
  push_private "$MANIFEST" model-manifest.json
  push_private "$CORPUS_MANIFEST" corpus-manifest.json

  shopt -s nullglob
  local wav reference name
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
  adb_cmd shell run-as "$PACKAGE" cat files/asr-benchmark/last-result.json
}

case "${1:-}" in
  prepare) prepare ;;
  build) build ;;
  install) install_apk ;;
  push) push_data ;;
  run) run_app ;;
  result) print_result ;;
  all)
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
