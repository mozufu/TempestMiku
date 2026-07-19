#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SOURCE="$ROOT/corpus/zh-tw-synthetic-v1.tsv"
CACHE="$ROOT/.asr-benchmark"
CORPUS_DIR="$CACHE/corpus"
CORPUS_MANIFEST="$CACHE/corpus-manifest.json"
VOICE="Meijia"
RATE=185

for tool in say ffmpeg ffprobe jq; do
  command -v "$tool" >/dev/null || {
    echo "missing required tool: $tool" >&2
    exit 1
  }
done

[[ "$(uname -s)" == "Darwin" ]] || {
  echo "the synthetic zh-TW generator requires the macOS say command" >&2
  exit 1
}
say -v '?' | grep "^$VOICE[[:space:]].*zh_TW" >/dev/null || {
  echo "missing macOS zh_TW voice: $VOICE" >&2
  exit 1
}
[[ -f "$SOURCE" ]] || { echo "missing $SOURCE" >&2; exit 1; }

mkdir -p "$CORPUS_DIR"
rm -f "$CORPUS_DIR"/zh-tw-synth-*.wav "$CORPUS_DIR"/zh-tw-synth-*.txt
temporary="$(mktemp -d "${TMPDIR:-/tmp}/tm-zh-tw-corpus.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT
cases_json="$temporary/cases.json"
printf '[]\n' >"$cases_json"

item_count=0
long_count=0
while IFS=$'\t' read -r id length category repeats transcript; do
  [[ -z "$id" || "$id" == \#* ]] && continue
  [[ "$id" =~ ^[0-9]{3}$ ]] || { echo "invalid id: $id" >&2; exit 1; }
  [[ "$length" == "short" || "$length" == "long" ]] || {
    echo "invalid length for $id: $length" >&2
    exit 1
  }
  [[ "$category" =~ ^[a-z-]+$ ]] || {
    echo "invalid category for $id: $category" >&2
    exit 1
  }
  [[ "$repeats" =~ ^[1-9][0-9]*$ ]] || {
    echo "invalid repeat count for $id: $repeats" >&2
    exit 1
  }
  [[ -n "$transcript" ]] || { echo "empty transcript for $id" >&2; exit 1; }

  utterance="$transcript"
  for ((index = 1; index < repeats; index++)); do
    utterance="$utterance $transcript"
  done

  basename="zh-tw-synth-$id-$category"
  raw="$temporary/$basename.aiff"
  output="$CORPUS_DIR/$basename.wav"
  reference="$CORPUS_DIR/$basename.txt"
  say -v "$VOICE" -r "$RATE" -o "$raw" "$utterance"

  if [[ "$length" == "long" ]]; then
    raw_duration="$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$raw")"
    tempo="$(awk -v duration="$raw_duration" 'BEGIN { printf "%.8f", duration / 59.5 }')"
    ffmpeg -nostdin -v error -y -i "$raw" -af "atempo=$tempo" \
      -ac 1 -ar 16000 -c:a pcm_s16le "$output"
    long_count=$((long_count + 1))
  else
    ffmpeg -nostdin -v error -y -i "$raw" \
      -ac 1 -ar 16000 -c:a pcm_s16le "$output"
  fi
  printf '%s\n' "$utterance" >"$reference"

  audio_bytes="$(wc -c <"$output" | tr -d '[:space:]')"
  audio_sha256="$(shasum -a 256 "$output" | awk '{print $1}')"
  reference_bytes="$(wc -c <"$reference" | tr -d '[:space:]')"
  reference_sha256="$(shasum -a 256 "$reference" | awk '{print $1}')"

  case_json="$temporary/cases.next.json"
  jq \
    --arg file "$(basename "$output")" \
    --arg reference "$(basename "$reference")" \
    --arg category "$category" \
    --argjson long "$([[ "$length" == "long" ]] && echo true || echo false)" \
    --argjson audio_bytes "$audio_bytes" \
    --arg audio_sha256 "$audio_sha256" \
    --argjson reference_bytes "$reference_bytes" \
    --arg reference_sha256 "$reference_sha256" \
    '. + [{
      file: $file,
      reference: $reference,
      category: $category,
      long: $long,
      audioBytes: $audio_bytes,
      audioSha256: $audio_sha256,
      referenceBytes: $reference_bytes,
      referenceSha256: $reference_sha256
    }]' \
    "$cases_json" >"$case_json"
  mv "$case_json" "$cases_json"

  duration="$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$output")"
  if [[ "$length" == "long" ]]; then
    awk -v duration="$duration" 'BEGIN { exit !(duration >= 59 && duration <= 60) }' || {
      echo "$basename duration is outside 59-60 seconds: $duration" >&2
      exit 1
    }
  else
    awk -v duration="$duration" 'BEGIN { exit !(duration > 0 && duration < 59) }' || {
      echo "$basename short duration is invalid: $duration" >&2
      exit 1
    }
  fi
  item_count=$((item_count + 1))
  printf '%s\t%s seconds\n' "$basename" "$duration"
done <"$SOURCE"

[[ "$item_count" == 50 ]] || {
  echo "expected 50 synthetic items, generated $item_count" >&2
  exit 1
}
[[ "$long_count" == 3 ]] || {
  echo "expected 3 long items, generated $long_count" >&2
  exit 1
}

jq -n \
  --arg source "corpus/$(basename "$SOURCE")" \
  --arg source_sha256 "$(shasum -a 256 "$SOURCE" | awk '{print $1}')" \
  --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg voice "$VOICE" \
  --argjson rate "$RATE" \
  --slurpfile cases "$cases_json" \
  '{
    schema: 3,
    id: "zh-tw-meijia-synthetic-v1",
    kind: "synthetic_tts",
    locale: "zh_TW",
    transcriptionScript: "Traditional Chinese",
    voice: $voice,
    rateWordsPerMinute: $rate,
    source: $source,
    sourceSha256: $source_sha256,
    generatedAt: $generated_at,
    items: 50,
    longItems: 3,
    targetLongDurationSeconds: 59.5,
    cases: $cases[0],
    limitations: [
      "Synthetic TTS is a deterministic performance and orthography probe, not human-speech accuracy evidence.",
      "It does not cover microphones, environmental noise, spontaneous speech, disfluencies, or speaker diversity.",
      "Strict CER intentionally counts Traditional and Simplified Chinese substitutions as errors."
    ]
  }' >"$CORPUS_MANIFEST"

echo "generated $item_count synthetic zh-TW items ($long_count long) in $CORPUS_DIR"
