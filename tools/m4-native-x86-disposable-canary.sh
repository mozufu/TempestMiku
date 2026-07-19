#!/usr/bin/env bash
set -euo pipefail

# This harness is intentionally a disposable architecture canary, not a
# production-service acceptance run.  The UID, GID, and architecture below are
# operator-authored literals; they must never be replaced with id(1) or uname(1)
# output when passed to the strict M4 wrapper.
readonly M4_SERVICE_UID=23017
readonly M4_SERVICE_GID=23017
readonly M4_SERVICE_ARCH=x86_64
readonly M4_BASE_IMAGE=rust:1.88-trixie
readonly M4_REPORT_NAME=m4-linux-hardened-v1-disposable-native-x86_64.json
readonly M4_CONTRACT_NAME=m4-linux-hardened-v1-disposable-native-x86_64.contract.json
readonly M4_HOST_CONFIG_NAME=m4-linux-hardened-v1-disposable-native-x86_64.host-config.json

fail() {
  printf 'm4 native x86_64 disposable canary: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

cleanup_outer_report() {
  if [[ "${outer_succeeded:-0}" != 1 ]]; then
    rm -f -- "${outer_report:-}" "${outer_contract:-}" "${outer_host_config:-}"
  fi
}

run_outer() {
  [[ "$#" == 1 ]] || fail "usage: $0 ABSOLUTE_OUTPUT_DIRECTORY"
  [[ "$(uname -s)" == Linux ]] || fail "the native workflow requires Linux"
  [[ "$(uname -m)" == x86_64 ]] || fail "the native workflow requires an x86_64 host"
  require_command docker
  require_command python3

  local output_directory="$1"
  [[ "$output_directory" == /* ]] || fail "output directory must be absolute"
  [[ -d "$output_directory" && ! -L "$output_directory" ]] \
    || fail "output directory must be an existing non-symlink directory"
  output_directory="$(cd "$output_directory" && pwd -P)"

  local script_directory repository_root image_id
  script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
  repository_root="$(cd "$script_directory/.." && pwd -P)"
  outer_report="$output_directory/$M4_REPORT_NAME"
  outer_contract="$output_directory/$M4_CONTRACT_NAME"
  outer_host_config="$output_directory/$M4_HOST_CONFIG_NAME"
  for candidate in "$outer_report" "$outer_contract" "$outer_host_config"; do
    [[ ! -e "$candidate" && ! -L "$candidate" ]] \
      || fail "refusing to overwrite $candidate"
  done
  outer_succeeded=0
  trap cleanup_outer_report EXIT

  docker pull --platform linux/amd64 "$M4_BASE_IMAGE"
  image_id="$(docker image inspect --format '{{.Id}}' "$M4_BASE_IMAGE")"
  [[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] \
    || fail "Docker returned an invalid immutable image identity"

  docker run --rm \
    --platform linux/amd64 \
    --privileged \
    --cgroupns host \
    --security-opt apparmor=unconfined \
    --security-opt seccomp=unconfined \
    --mount "type=bind,src=$repository_root,dst=/workspace,readonly" \
    --mount "type=bind,src=$output_directory,dst=/evidence" \
    --mount "type=bind,src=/sys/fs/cgroup,dst=/sys/fs/cgroup" \
    --env "TM_M4_BASE_IMAGE_IDENTITY=docker-image@$image_id" \
    "$image_id" \
    /bin/bash /workspace/tools/m4-native-x86-disposable-canary.sh --inside

  for candidate in "$outer_report" "$outer_contract" "$outer_host_config"; do
    [[ -f "$candidate" && ! -L "$candidate" ]] \
      || fail "the successful container did not publish $candidate"
  done
  local expected_revision expected_artifact
  expected_revision="$(python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["source"]["expectedRevision"])' \
    "$outer_contract")"
  expected_artifact="$(python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["service"]["runtimeArtifactIdentity"])' \
    "$outer_contract")"
  python3 "$repository_root/tools/m4_acceptance.py" validate-report "$outer_report" \
    --expected-contract "$outer_contract" \
    --expected-host-config "$outer_host_config" \
    --expected-revision "$expected_revision" \
    --expected-runtime-artifact "$expected_artifact"

  outer_succeeded=1
  trap - EXIT
  printf 'M4 disposable native x86_64 report: %s\n' "$outer_report"
}

cleanup_inner() {
  local status=$?
  set +e

  if [[ "${inner_succeeded:-0}" != 1 ]]; then
    rm -f -- \
      "${publish_report_temporary:-}" \
      "${publish_contract_temporary:-}" \
      "${publish_host_config_temporary:-}" \
      "${published_report:-}" \
      "${published_contract:-}" \
      "${published_host_config:-}"
  fi

  # The service process runs below the delegated root so it can move its children into
  # per-run leaves without needing write authority over the container's parent cgroup.
  # Move this root-owned manager shell back out before killing/removing the delegated tree.
  if [[ -n "${manager_cgroup:-}" && -d "$manager_cgroup" ]]; then
    printf '%s\n' "$$" > "$manager_cgroup/cgroup.procs"
  fi

  if [[ -n "${delegated_cgroup:-}" && -d "$delegated_cgroup" ]]; then
    if [[ -w "$delegated_cgroup/cgroup.kill" ]]; then
      printf '1\n' > "$delegated_cgroup/cgroup.kill"
    fi
    while IFS= read -r child; do
      if [[ -w "$child/cgroup.subtree_control" ]]; then
        printf '%s\n' '-cpu -memory -pids' > "$child/cgroup.subtree_control"
      fi
      rmdir -- "$child"
    done < <(find "$delegated_cgroup" -mindepth 1 -type d -depth -print)
    printf '%s\n' '-cpu -memory -pids' > "$delegated_cgroup/cgroup.subtree_control"
    rmdir -- "$delegated_cgroup"
  fi

  if [[ -n "${parent_cgroup:-}" && -d "$parent_cgroup" ]]; then
    printf '%s\n' '-cpu -memory -pids' > "$parent_cgroup/cgroup.subtree_control"
  fi
  if [[ -n "${manager_cgroup:-}" && -d "$manager_cgroup" ]]; then
    local pid
    while IFS= read -r pid; do
      [[ -n "$pid" ]] && printf '%s\n' "$pid" > "$parent_cgroup/cgroup.procs"
    done < "$manager_cgroup/cgroup.procs"
    rmdir -- "$manager_cgroup"
  fi

  exit "$status"
}

install_runtime() {
  local runtime_root="$1"
  local busybox_source probe_source resource_probe_source

  install -d -o root -g root -m 0755 "$runtime_root/bin"
  busybox_source="$(command -v busybox)"
  install -o root -g root -m 0755 "$busybox_source" "$runtime_root/bin/busybox"
  install -o root -g root -m 0755 /usr/bin/bwrap "$runtime_root/bin/bwrap"
  local applet
  for applet in cat env mount sh sleep test touch true unshare wget; do
    ln -s busybox "$runtime_root/bin/$applet"
  done

  probe_source=/workspace/tools/m4-thread-probe.c
  [[ -f "$probe_source" && ! -L "$probe_source" ]] \
    || fail "the M4 thread probe source is missing"
  gcc -O2 -static -pthread "$probe_source" -o "$runtime_root/bin/thread-probe"
  chown root:root "$runtime_root/bin/thread-probe"
  chmod 0755 "$runtime_root/bin/thread-probe"
  resource_probe_source=/workspace/tools/m4-resource-probe.c
  [[ -f "$resource_probe_source" && ! -L "$resource_probe_source" ]] \
    || fail "the M4 resource probe source is missing"
  gcc -O2 -static "$resource_probe_source" -o "$runtime_root/bin/resource-probe"
  chown root:root "$runtime_root/bin/resource-probe"
  chmod 0755 "$runtime_root/bin/resource-probe"
  chmod -R go-w "$runtime_root"
}

prepare_cgroup_delegation() {
  local cgroup_mount=/sys/fs/cgroup current_relative original_subtree child pid
  [[ "$(stat -f -c %T "$cgroup_mount")" == cgroup2fs ]] \
    || fail "/sys/fs/cgroup is not a writable cgroup-v2 mount"
  current_relative="$(awk -F: '$1 == "0" { print $3 }' /proc/self/cgroup)"
  [[ -n "$current_relative" && "$current_relative" != / ]] \
    || fail "refusing to prepare delegation at the cgroup-v2 mount root"
  parent_cgroup="$cgroup_mount$current_relative"
  [[ -d "$parent_cgroup" && -w "$parent_cgroup/cgroup.procs" ]] \
    || fail "the disposable container cgroup is not writable"
  original_subtree="$(<"$parent_cgroup/cgroup.subtree_control")"
  [[ -z "$original_subtree" ]] \
    || fail "the disposable container cgroup already delegates controllers"
  child="$(find "$parent_cgroup" -mindepth 1 -maxdepth 1 -type d -print -quit)"
  [[ -z "$child" ]] || fail "the disposable container cgroup already has child cgroups"

  manager_cgroup="$parent_cgroup/tm-m4-manager-v1"
  delegated_cgroup="$parent_cgroup/tm-m4-delegated-v1"
  mkdir -- "$manager_cgroup" "$delegated_cgroup"

  local attempts=0
  local -a resident_pids=()
  while (( attempts < 20 )); do
    mapfile -t resident_pids < "$parent_cgroup/cgroup.procs"
    ((${#resident_pids[@]} == 0)) && break
    for pid in "${resident_pids[@]}"; do
      [[ -n "$pid" ]] && printf '%s\n' "$pid" > "$manager_cgroup/cgroup.procs" || true
    done
    attempts=$((attempts + 1))
  done
  mapfile -t resident_pids < "$parent_cgroup/cgroup.procs"
  ((${#resident_pids[@]} == 0)) \
    || fail "could not empty the disposable container cgroup before delegation"

  local available
  available=" $(<"$parent_cgroup/cgroup.controllers") "
  for child in cpu memory pids; do
    [[ "$available" == *" $child "* ]] \
      || fail "the disposable container cgroup lacks the $child controller"
  done
  printf '%s\n' '+cpu +memory +pids' > "$parent_cgroup/cgroup.subtree_control"
  printf '%s\n' '+cpu +memory +pids' > "$delegated_cgroup/cgroup.subtree_control"

  # A delegated cgroup with controllers enabled cannot itself contain processes. Keep the
  # service process in one child and give tm-host a separate empty child whose controllers are
  # enabled for per-run leaves. Their common ancestor remains service-writable, as required by
  # cgroup v2's migration containment rule.
  service_cgroup="$delegated_cgroup/tm-m4-service-v1"
  runtime_cgroup="$delegated_cgroup/tm-m4-runtime-v1"
  mkdir -- "$service_cgroup" "$runtime_cgroup"
  printf '%s\n' '+cpu +memory +pids' > "$runtime_cgroup/cgroup.subtree_control"

  chown "$M4_SERVICE_UID:$M4_SERVICE_GID" \
    "$delegated_cgroup" \
    "$delegated_cgroup/cgroup.procs" \
    "$delegated_cgroup/cgroup.threads" \
    "$delegated_cgroup/cgroup.subtree_control" \
    "$delegated_cgroup/cgroup.kill" \
    "$service_cgroup" \
    "$service_cgroup/cgroup.procs" \
    "$service_cgroup/cgroup.threads" \
    "$service_cgroup/cgroup.kill" \
    "$runtime_cgroup" \
    "$runtime_cgroup/cgroup.procs" \
    "$runtime_cgroup/cgroup.threads" \
    "$runtime_cgroup/cgroup.subtree_control" \
    "$runtime_cgroup/cgroup.kill"
}

run_inside() {
  [[ "$#" == 0 ]] || fail "--inside does not accept arguments"
  [[ "${EUID:-1}" == 0 ]] || fail "the disposable container setup must run as root"
  [[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] \
    || fail "the disposable container must run natively on Linux x86_64"
  [[ "${TM_M4_BASE_IMAGE_IDENTITY:-}" =~ ^docker-image@sha256:[0-9a-f]{64}$ ]] \
    || fail "an immutable base-image identity is required"
  [[ -d /workspace && -d /evidence ]] || fail "workspace/evidence mounts are missing"

  published_report="/evidence/$M4_REPORT_NAME"
  published_contract="/evidence/$M4_CONTRACT_NAME"
  published_host_config="/evidence/$M4_HOST_CONFIG_NAME"
  publish_report_temporary="/evidence/.$M4_REPORT_NAME.$$.tmp"
  publish_contract_temporary="/evidence/.$M4_CONTRACT_NAME.$$.tmp"
  publish_host_config_temporary="/evidence/.$M4_HOST_CONFIG_NAME.$$.tmp"
  for candidate in "$published_report" "$published_contract" "$published_host_config"; do
    [[ ! -e "$candidate" && ! -L "$candidate" ]] \
      || fail "refusing to overwrite $candidate"
  done
  inner_succeeded=0
  parent_cgroup=
  manager_cgroup=
  delegated_cgroup=
  service_cgroup=
  runtime_cgroup=
  trap cleanup_inner EXIT

  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends \
    bubblewrap \
    build-essential \
    busybox-static \
    ca-certificates \
    git \
    python3 \
    util-linux
  rm -rf /var/lib/apt/lists/*

  local runtime_root=/opt/tempestmiku-isolation-runtime
  local linked_root=/srv/tempestmiku-linked
  local state_root=/var/lib/tempestmiku-m4-disposable
  local cargo_home=/tmp/tempestmiku-m4-cargo-home
  local cargo_target=/tmp/tempestmiku-m4-target
  local service_home="$state_root/home"
  local state_directory="$state_root/tm-m4-acceptance-native-x86_64"
  local kit_root=/opt/tempestmiku-m4-kit
  local host_config=/etc/tempestmiku/m4-host.json
  local deployment_contract=/etc/tempestmiku/m4-deployment-contract.json
  local service_manager_config=/etc/systemd/system/tempestmiku-m4-disposable.service
  local server_binary=/usr/local/libexec/tempestmiku/tm-server
  local evidence_output="$state_root/evidence/$M4_REPORT_NAME"
  local service_path="$runtime_root/bin"
  local build_messages=/tmp/tempestmiku-m4-build-messages.jsonl
  local canary_binary
  local python_binary
  local expected_revision
  local server_sha

  install_runtime "$runtime_root"
  install -d -o "$M4_SERVICE_UID" -g "$M4_SERVICE_GID" -m 0750 "$state_root"
  install -d -o "$M4_SERVICE_UID" -g "$M4_SERVICE_GID" -m 0750 \
    "$linked_root" "$state_root/evidence" "$service_home"
  install -d -o "$M4_SERVICE_UID" -g "$M4_SERVICE_GID" -m 0700 "$state_directory"
  install -d -o root -g root -m 0755 \
    /etc/tempestmiku /etc/systemd/system /usr/local/libexec/tempestmiku "$kit_root"
  install -d -o root -g root -m 0755 "$cargo_home" "$cargo_target"

  env -i \
    PATH=/usr/local/cargo/bin:/usr/bin:/bin \
    HOME=/root \
    CARGO_HOME="$cargo_home" \
    RUSTUP_HOME=/usr/local/rustup \
    /usr/local/cargo/bin/cargo fetch \
      --manifest-path /workspace/Cargo.toml \
      --locked
  env -i \
    PATH=/usr/local/cargo/bin:/usr/bin:/bin \
    HOME=/root \
    CARGO_HOME="$cargo_home" \
    CARGO_TARGET_DIR="$cargo_target" \
    RUSTUP_HOME=/usr/local/rustup \
    /usr/local/cargo/bin/cargo test \
      --manifest-path /workspace/Cargo.toml \
      --offline \
      --frozen \
      -p tm-host \
      --no-run \
      --message-format=json > "$build_messages"
  canary_binary="$(python3 -c '
import json, pathlib, sys
matches=[]
for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    value=json.loads(line)
    target=value.get("target", {})
    executable=value.get("executable")
    if value.get("reason") == "compiler-artifact" and target.get("name") == "tm_host" and target.get("test") and executable:
        matches.append(executable)
if len(matches) != 1:
    raise SystemExit(f"expected one tm_host test executable, got {matches!r}")
print(matches[0])
' "$build_messages")"
  [[ "$canary_binary" == /* && -x "$canary_binary" ]] \
    || fail "offline/frozen build did not produce one executable tm-host canary"
  env -i \
    PATH=/usr/local/cargo/bin:/usr/bin:/bin \
    HOME=/root \
    CARGO_HOME="$cargo_home" \
    CARGO_TARGET_DIR="$cargo_target" \
    RUSTUP_HOME=/usr/local/rustup \
    /usr/local/cargo/bin/cargo build \
      --manifest-path /workspace/Cargo.toml \
      --offline \
      --frozen \
      -p tm-server
  install -o root -g root -m 0755 "$cargo_target/debug/tm-server" "$server_binary"
  install -o root -g root -m 0755 "$canary_binary" "$kit_root/tm-host-canary"
  install -o root -g root -m 0755 \
    /workspace/tools/m4-linux-hardened-canary.sh \
    "$kit_root/m4-linux-hardened-canary.sh"
  install -o root -g root -m 0644 \
    /workspace/tools/m4_acceptance.py \
    "$kit_root/m4_acceptance.py"

  chmod -R go-w /usr/local/cargo /usr/local/rustup
  prepare_cgroup_delegation

  cat > "$host_config" <<EOF
{
  "linked_folders": [
    {
      "name": "tempestmiku",
      "path": "$linked_root",
      "mode": "rw",
      "commands": ["cat", "env", "mount", "sh", "sleep", "test", "thread-probe", "touch", "unshare", "wget"],
      "safe_args": []
    }
  ],
  "proc_isolation": {
    "provider": "linux_hardened_v1",
    "launcher": "$runtime_root/bin/bwrap",
    "runtime_roots": ["$runtime_root"],
    "limits": {
      "address_space_bytes": 2147483648,
      "process_count": 64,
      "open_files": 256
    },
    "cgroup_root": "$runtime_cgroup",
    "cgroup_limits": {
      "memory_max_bytes": 1073741824,
      "memory_swap_max_bytes": 0,
      "pids_max": 64,
      "cpu_quota_micros": 100000,
      "cpu_period_micros": 100000
    }
  }
}
EOF

  cat > "$service_manager_config" <<EOF
[Unit]
Description=TempestMiku disposable M4 native x86_64 acceptance identity

[Service]
User=$M4_SERVICE_UID
Group=$M4_SERVICE_GID
Environment=TM_HOST_CONFIG=$host_config
Environment=TM_SERVER_ROLE=all
ExecStart=$server_binary
EOF

  python_binary="$(readlink -f /usr/bin/python3)"
  [[ "$python_binary" == /* && -x "$python_binary" && ! -L "$python_binary" ]] \
    || fail "could not resolve a regular absolute Python interpreter"
  expected_revision="$(/usr/bin/git --no-optional-locks \
    -c safe.directory=/workspace \
    -c core.hooksPath=/dev/null \
    -C /workspace rev-parse HEAD)"
  [[ "$expected_revision" =~ ^[0-9a-f]{40}$ ]] \
    || fail "could not resolve the exact clean source revision"
  server_sha="$(python3 -c \
    'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' \
    "$server_binary")"

  cat > "$deployment_contract" <<EOF
{
  "schemaVersion": 1,
  "evidenceClass": "disposable_native_architecture",
  "threatModel": {
    "hostKernel": "trusted",
    "workload": "hostile",
    "hostileHostKernelClaimed": false
  },
  "microvm": {
    "required": false,
    "selected": false,
    "provider": null,
    "evidencePath": null
  },
  "service": {
    "name": "tempestmiku-m4-disposable-native-x86_64",
    "role": "all",
    "expectedUid": $M4_SERVICE_UID,
    "expectedGid": $M4_SERVICE_GID,
    "expectedArchitecture": "$M4_SERVICE_ARCH",
    "hostConfigPath": "$host_config",
    "serviceManagerConfigPath": "$service_manager_config",
    "serverBinaryPath": "$server_binary",
    "runtimeArtifactIdentity": "tm-server@sha256:$server_sha",
    "expectedEnvironment": {
      "TM_HOST_CONFIG": "$host_config",
      "TM_SERVER_ROLE": "all"
    }
  },
  "isolation": {
    "provider": "linux_hardened_v1",
    "cgroupRoot": "$runtime_cgroup",
    "launcher": "$runtime_root/bin/bwrap",
    "runtimeRoots": ["$runtime_root"],
    "linkedRoots": [
      {
        "path": "$linked_root",
        "mode": "rw"
      }
    ],
    "exclusivity": {
      "status": "not_proven",
      "reason": "Disposable empty-root observation only; no production supervisor lease or unique binding is represented."
    }
  },
  "tools": {
    "python": {
      "path": "$python_binary",
      "sha256": "$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$python_binary")"
    },
    "git": {
      "path": "/usr/bin/git",
      "sha256": "$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' /usr/bin/git)"
    },
    "bash": {
      "path": "/bin/bash",
      "sha256": "$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' /bin/bash)"
    },
    "wrapper": {
      "path": "$kit_root/m4-linux-hardened-canary.sh",
      "sha256": "$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$kit_root/m4-linux-hardened-canary.sh")"
    },
    "acceptanceProgram": {
      "path": "$kit_root/m4_acceptance.py",
      "sha256": "$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$kit_root/m4_acceptance.py")"
    },
    "canaryExecutable": {
      "path": "$kit_root/tm-host-canary",
      "sha256": "$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$kit_root/tm-host-canary")"
    }
  },
  "execution": {
    "timeoutSeconds": 300,
    "environment": {
      "PATH": "$service_path",
      "HOME": "$service_home",
      "TMPDIR": "$state_directory"
    }
  },
  "source": {
    "repositoryRoot": "/workspace",
    "expectedRevision": "$expected_revision",
    "requireClean": true
  },
  "workloadSizing": {
    "status": "pending",
    "reason": "Disposable native-architecture canary only; representative memory.peak, pids.peak, and production headroom are not measured."
  }
}
EOF
  chown root:root "$host_config" "$deployment_contract" "$service_manager_config"
  chmod 0644 "$host_config" "$deployment_contract" "$service_manager_config"

  # Keep the service and its canary child inside the delegated boundary before dropping UID.
  # tm-host can then migrate each child into a runtime leaf without authority over the outer
  # container cgroup.
  printf '%s\n' "$$" > "$service_cgroup/cgroup.procs"

  /usr/bin/setpriv \
    --reuid="$M4_SERVICE_UID" \
    --regid="$M4_SERVICE_GID" \
    --clear-groups \
    /usr/bin/env -i \
      TM_M4_EXPECTED_UID=23017 \
      TM_M4_EXPECTED_GID=23017 \
      TM_M4_EXPECTED_ARCH=x86_64 \
      TM_HOST_CONFIG="$host_config" \
      TM_M4_DEPLOYMENT_CONTRACT="$deployment_contract" \
      TM_M4_EVIDENCE_OUTPUT="$evidence_output" \
      TM_M4_STATE_DIRECTORY="$state_directory" \
      TM_M4_PYTHON="$python_binary" \
      TM_M4_BASH=/bin/bash \
      TM_M4_ACCEPTANCE_PROGRAM="$kit_root/m4_acceptance.py" \
      "$kit_root/m4-linux-hardened-canary.sh"

  "$python_binary" "$kit_root/m4_acceptance.py" validate-report "$evidence_output" \
    --expected-contract "$deployment_contract" \
    --expected-host-config "$host_config" \
    --expected-revision "$expected_revision" \
    --expected-runtime-artifact "tm-server@sha256:$server_sha"

  install -o root -g root -m 0644 "$evidence_output" "$publish_report_temporary"
  install -o root -g root -m 0644 "$deployment_contract" "$publish_contract_temporary"
  install -o root -g root -m 0644 "$host_config" "$publish_host_config_temporary"
  mv -T -- "$publish_report_temporary" "$published_report"
  mv -T -- "$publish_contract_temporary" "$published_contract"
  mv -T -- "$publish_host_config_temporary" "$published_host_config"
  inner_succeeded=1
  printf 'Published disposable native x86_64 evidence bundle: %s\n' "$published_report"
}

if [[ "${1:-}" == --inside ]]; then
  shift
  run_inside "$@"
else
  run_outer "$@"
fi
