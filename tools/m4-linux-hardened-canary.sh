#!/bin/bash
set -euo pipefail

fail() {
  printf 'm4 hardened canary: %s\n' "$*" >&2
  exit 1
}

require_env() {
  local name="$1"
  [[ -n "${!name:-}" ]] || fail "$name is required"
}

require_env TM_M4_EXPECTED_UID
require_env TM_M4_EXPECTED_GID
require_env TM_M4_EXPECTED_ARCH
require_env TM_HOST_CONFIG
require_env TM_M4_DEPLOYMENT_CONTRACT
require_env TM_M4_EVIDENCE_OUTPUT
require_env TM_M4_STATE_DIRECTORY
require_env TM_M4_PYTHON
require_env TM_M4_BASH
require_env TM_M4_ACCEPTANCE_PROGRAM

readonly wrapper_path="${BASH_SOURCE[0]}"
[[ "$wrapper_path" == /* ]] || fail "the pinned wrapper must be invoked by absolute path"
[[ "$TM_M4_PYTHON" == /* ]] || fail "TM_M4_PYTHON must be absolute"
[[ "$TM_M4_BASH" == /* ]] || fail "TM_M4_BASH must be absolute"
[[ "$TM_M4_ACCEPTANCE_PROGRAM" == /* ]] \
  || fail "TM_M4_ACCEPTANCE_PROGRAM must be absolute"
[[ "$TM_M4_STATE_DIRECTORY" == /* ]] || fail "TM_M4_STATE_DIRECTORY must be absolute"
[[ "$TM_M4_EVIDENCE_OUTPUT" == /* ]] || fail "TM_M4_EVIDENCE_OUTPUT must be absolute"
[[ -d "$TM_M4_STATE_DIRECTORY" && ! -L "$TM_M4_STATE_DIRECTORY" ]] \
  || fail "private state directory is missing or is a symlink"
[[ ! -e "$TM_M4_EVIDENCE_OUTPUT" && ! -L "$TM_M4_EVIDENCE_OUTPUT" ]] \
  || fail "refusing to overwrite existing evidence"

readonly state_file="$TM_M4_STATE_DIRECTORY/preflight.json"
readonly canary_output="$TM_M4_STATE_DIRECTORY/canary-output.bin"

cleanup() {
  local status=$?
  set +e
  "$TM_M4_PYTHON" "$TM_M4_ACCEPTANCE_PROGRAM" cleanup \
    --state-directory "$TM_M4_STATE_DIRECTORY"
  local cleanup_status=$?
  if [[ "$status" -eq 0 && "$cleanup_status" -ne 0 ]]; then
    status="$cleanup_status"
  fi
  exit "$status"
}
trap cleanup EXIT

printf '%s\n' \
  "M4 evidence class: disposable native architecture only" \
  "M4 threat boundary: hostile workload on a trusted host kernel" \
  "M4 open boundaries: production service, cgroup exclusivity, workload sizing, x86 evidence, hostile host kernel, and microVM assurance"

"$TM_M4_PYTHON" "$TM_M4_ACCEPTANCE_PROGRAM" preflight \
  --contract "$TM_M4_DEPLOYMENT_CONTRACT" \
  --host-config "$TM_HOST_CONFIG" \
  --expected-uid "$TM_M4_EXPECTED_UID" \
  --expected-gid "$TM_M4_EXPECTED_GID" \
  --expected-arch "$TM_M4_EXPECTED_ARCH" \
  --python "$TM_M4_PYTHON" \
  --bash "$TM_M4_BASH" \
  --wrapper "$wrapper_path" \
  --acceptance-program "$TM_M4_ACCEPTANCE_PROGRAM" \
  --state-output "$state_file"

"$TM_M4_PYTHON" "$TM_M4_ACCEPTANCE_PROGRAM" run-canary \
  --state "$state_file" \
  --output "$canary_output"

"$TM_M4_PYTHON" "$TM_M4_ACCEPTANCE_PROGRAM" finalize \
  --state "$state_file" \
  --canary-output "$canary_output" \
  --output "$TM_M4_EVIDENCE_OUTPUT"

printf '%s\n' \
  "m4 disposable hardened canary passed" \
  "evidence=$TM_M4_EVIDENCE_OUTPUT"
