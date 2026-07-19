# M4 `linux_hardened_v1` software gate — 2026-07-18

## Result

The repo-owned higher-assurance `proc.run` profile passed the retained 2026-07-18 disposable Linux
canary at that recorded source revision: `1 passed; 0 failed; 82 filtered out`. The 2026-07-19
acceptance-kit hardening below changes the gate runner and has not produced a replacement live
native report. The retained run is a software/disposable result, not a production host or microVM
closeout. Its machine-readable companion is
[`2026-07-18-m4-linux-hardened-v1.json`](2026-07-18-m4-linux-hardened-v1.json).

The precise `linux_hardened_v1` threat model is a hostile workload on a trusted host kernel. It does
not contain a hostile host kernel. If the host kernel is in the threat model, a separate-kernel
microVM is mandatory and needs its own selected-target evidence; neither the disposable container
run nor a v1 acceptance report claims that assurance.

`ProcIsolationConfig::LinuxHardenedV1` is opt-in and has no fallback. Before approval it requires a
pinned root-owned bubblewrap/runtime profile, the sealed architecture-specific `developer_v1`
seccomp program, and a writable pinned cgroup-v2 delegated subtree with `cpu`, `memory`, `pids`,
`cgroup.kill`, and child controller delegation. The approval/profile digest binds launcher identity,
runtime source/destination mounts, rlimits, seccomp version/architecture/content digest, cgroup-root
device/inode, and all cgroup limits.

The fixed classic-BPF policy is 560 bytes on both supported architectures. Its frozen digests are:

- `x86_64`: `5f5a85e74abb372634d8a0bc05bc9c29aaf361803c7b2eaed49af11d3ee22487`
- `aarch64`: `a0199be3c1a1aa40b387946bd05ae72771f44119074e84cad4b118b565a9d60a`

It denies mount/topology changes, namespace creation, BPF, perf, keyring, module, kexec, reboot,
swap, ptrace, cross-process memory, and userfaultfd syscalls. Ordinary `clone` remains available
unless namespace flags are present; opaque `clone3` returns `ENOSYS` so libc can safely fall back.
The policy is passed to bubblewrap through a sealed, descriptor-pinned `--add-seccomp-fd`.

Every execution creates an unpredictable `tm-run-v1-*` leaf, writes and reads back `memory.max`,
`memory.swap.max`, `pids.max`, and `cpu.max`, then joins the pre-exec child through a pinned
`cgroup.procs` descriptor. Success, timeout, cancellation, and drop all use `cgroup.kill`, wait for
`populated 0`, read memory/pids/CPU counters, and remove the leaf. The explicit
`recover_orphans_at_startup` API deterministically kills and removes only exact service-owned leaf
names. `tm-server` now calls it before constructing either the API or worker runtime, fails startup
if recovery cannot complete, and logs only the provider, delegated root, and recovered-leaf count.
Concurrent instances still require distinct service-manager-delegated roots.

The deployment config boundary also rejects unknown top-level, linked-folder, approval,
self-evolution, egress, and isolation fields. In particular, a misspelled `proc_isolation` key can
no longer silently select the disabled profile.

## Canary coverage

The original privileged Docker canary used a private cgroup namespace. The final-source rerun used
the same Debian trixie Linux/aarch64 kernel and bubblewrap `0.11.0`, with a dedicated sibling
cgroup-v2 subtree in the disposable container host namespace so the test process was outside the
delegated subtree. That explicitly named subtree was empty before the run and removed afterward.
Both runs used static BusyBox probes and the normal `tm-host` `proc.run` approval path. They proved:

- a dependency-free `cargo test --offline` completed inside the sandbox;
- a Rust helper created and joined a real thread and spawned a child process;
- representative `unshare` and `mount` probes were denied;
- active leaf limits matched the configured memory/swap/pids/CPU values;
- successful, timed-out, and cancelled runs left no execution leaf;
- startup recovery killed a real process placed in a simulated crash orphan and removed the leaf;
- approval exposed the bound seccomp/cgroup metadata; and
- a missing delegated root failed before approval and did not create a fallback marker.

The runtime-root implementation also preserves configured sandbox destinations separately from
canonical trusted sources (for example `/lib -> /usr/lib`), and includes both in the profile digest.
This is required for dynamic linkers while retaining canonical source identity.

Focused verification:

```text
cargo test -p tm-host --no-fail-fast
79 passed; 0 failed

cargo fmt --all --check
passed

cargo clippy --workspace --all-targets --all-features -- -D warnings
passed on macOS after integration

TM_LINUX_HARDENED_TESTS=1 cargo test -p tm-host \
  linked::tests::linux_isolation::gated_linux_hardened_v1_enforces_seccomp_and_cgroup_lifecycle \
  -- --exact --test-threads=1 --nocapture
1 passed; 0 failed; 82 filtered out
```

## Acceptance kit v1 follow-up — 2026-07-19

The hardened wrapper is
[`tools/m4-linux-hardened-canary.sh`](../../tools/m4-linux-hardened-canary.sh). Acceptance-kit v1 now
has only one representable evidence class: `disposable_native_architecture`. Both the strict
validator and [schemas](../../tools/m4-acceptance-report.schema.json) reject
`production_service`. A disposable service-manager fixture can therefore exercise the code path but
cannot close production service, supervisor, role/environment, or production-host acceptance.

The wrapper accepts a root-owned [`deployment contract`](../../tools/m4-deployment-contract.example.json)
plus exact `TM_HOST_CONFIG` and literal operator-authored UID, GID, and architecture values. The
contract pins absolute paths and SHA-256 digests for Python, Git, Bash, the installed wrapper,
validator, and a prebuilt `tm-host` test executable. The acceptance run itself never invokes Cargo.
The disposable builder fetches dependencies first, then produces the final server and test binaries
under an empty environment with `--offline --frozen`; the wrapper executes only the pinned prebuilt
test with an exact argument vector.

Preflight requires a clean operator-pinned full Git revision. It verifies regular root-owned
non-symlink trusted files, the exact server-binary/runtime-artifact digest, bounded fixture
role/environment directives, final host config, linked-root path plus access mode, launcher, and a
recursive content manifest for every runtime root. It observes that the canary process is outside a
currently empty service-owned cgroup-v2 root and records namespace, controller and limit state. That
empty-before/after observation is **not** a production exclusivity proof: v1 has no supervisor lease
or unique service binding, so both contract and report must retain `exclusivity.status=not_proven`.

The prebuilt canary receives an exact five-key environment (`HOME`, `PATH`, `TMPDIR`,
`TM_HOST_CONFIG`, and `TM_LINUX_HARDENED_TESTS`), excluding ambient Cargo/Rust wrapper flags and
secrets. A Python process-group watchdog enforces a maximum 600-second contract bound while output
is streamed and capped at 1 MiB. Raw output is deleted with private state and is never retained in
the report; only byte count and SHA-256 plus the matched success assertion survive.

Only then is a v1 JSON report atomically linked and read back. Its complete canonical
`bindingSha256` detects mutation, while the `validate-report` command additionally requires an
externally supplied expected contract, host config, Git revision, and runtime artifact. A post-link
validation or directory-fsync failure removes the linked report. Any preflight/canary/identity/tree/
source/config failure writes no report and an existing path is never overwritten. Portable
adversarial regressions cover production/exclusivity overclaims, tool substitution, dirty source,
ambient Cargo/secret injection, runtime-tree mutation, expected-value mismatch, raw-output
retention, complete-report tampering, and post-link cleanup:

```text
python3 tools/m4_acceptance.py validate-contract tools/m4-deployment-contract.example.json
M4 disposable deployment contract valid

python3 -m unittest discover -s tools/tests -p 'test_m4_acceptance.py'
23 tests passed

bash -n tools/m4-linux-hardened-canary.sh
bash -n tools/m4-native-x86-disposable-canary.sh
passed

nix shell nixpkgs#cargo-audit nixpkgs#actionlint --command \
  actionlint .github/workflows/m4-native-x86-canary.yml
passed

docker run --rm --platform linux/arm64 -v <workspace>:/workspace:ro -w /workspace \
  -e CARGO_TARGET_DIR=/tmp/tempestmiku-m4-check-target rust:1.88-trixie \
  cargo check -p tm-host --tests --locked
passed (Linux/aarch64 compile-only; not a live isolation or deployment gate)

python3 tools/m4_acceptance.py validate-report /path/to/live-success-report.json \
  --expected-contract /path/to/expected-contract.json \
  --expected-host-config /path/to/expected-host-config.json \
  --expected-revision <40-hex-reviewed-revision> \
  --expected-runtime-artifact tm-server@sha256:<64-hex>
M4 contract-bound disposable report valid
```

The first two commands validate the kit, not a deployment. Only a contract/config/report bundle
emitted by a live successful wrapper run on the named native target qualifies as disposable target
evidence. Acceptance-kit v1 cannot emit a production report. A native x86_64 disposable bundle is
now retained below; it remains deliberately distinct from production acceptance.

Remaining external selections and measurements are explicit:

- the production Linux host/image, service manager, `api`/`worker`/`all` role, and a distinct
  non-root UID/GID for each concurrent instance;
- the selected native production architecture and immutable runtime artifact digest, final
  `tm-server` binary, host config, service-manager config, launcher, and runtime-root paths;
- the curated linked project roots and access modes, plus a supervisor-enforced unique lease/binding
  for one delegated cgroup-v2 root per instance in the final namespace;
- representative workload scenarios and retained `memory.peak`, `pids.peak`, CPU/timeout, failure,
  and concurrency measurements from which limits and headroom are chosen;
- whether the host kernel is trusted. If it is hostile/in scope, the microVM provider, guest image/
  kernel identity, service placement, and separate microVM evidence are mandatory selections.

### Native x86_64 disposable evidence passes

The manual-only
[`M4 disposable native x86_64 canary`](../../.github/workflows/m4-native-x86-canary.yml) provisions
a fresh cgroup-v2 delegation on an Ubuntu x86_64 runner and invokes the same strict wrapper through
[`tools/m4-native-x86-disposable-canary.sh`](../../tools/m4-native-x86-disposable-canary.sh). It is
`workflow_dispatch` only. It builds the final test and server artifacts in a sanitized
offline/frozen pass, installs the root-owned pinned kit, and publishes a report plus exact contract
and host-config sidecars. Any failed run removes/refuses the bundle and uploads nothing. The workflow
and harness being present is not evidence. On 2026-07-19 the owner authorized the same harness on
homolab, a native Linux `7.0.10` x86_64 host. A secrets-scanned clean ephemeral source snapshot was
transferred with SHA-256 verification. The first run exposed a real harness topology defect: the
service started outside the delegated common ancestor, so cgroup-v2 denied child migration. The
harness was corrected to place service and runtime roots as siblings below one delegated root and
to make the disposable state parent service-owned. A fresh container then passed the real focused
`linux_hardened_v1` test, bounded canary, report finalization, publication, and independent local
validation. The container and all `tm-m4-*` cgroups were absent afterward.

Retained bundle:

- [report](2026-07-19-m4-linux-hardened-v1-disposable-native-x86_64.json), SHA-256
  `775f2806e6234400507f5c3281a374748df6bb96f4a935e4cbbf536f042e05dc`;
- [contract](2026-07-19-m4-linux-hardened-v1-disposable-native-x86_64.contract.json), SHA-256
  `12a59a547dc3506d3b87d7ae86273c143695531607543853ca460d9a8f6096f6`; and
- [host config](2026-07-19-m4-linux-hardened-v1-disposable-native-x86_64.host-config.json), SHA-256
  `d37950e0bdb5fd3ff3fc7ae3b9087219a197f7ce781d070f874596e8cdd72eb0`.

The bundle binds ephemeral clean-snapshot revision
`5455f14ecd7552277d6bd67e4a3f613acac7e2c5`, immutable base image
`docker-image@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db`,
and `tm-server@sha256:250c7ba11e4ba1a6f8ca6d622cd168220845e43dece7ae34470ecd6e6fdd7452`.
It revalidates with:

```sh
python3 tools/m4_acceptance.py validate-report \
  docs/evidence/2026-07-19-m4-linux-hardened-v1-disposable-native-x86_64.json \
  --expected-contract docs/evidence/2026-07-19-m4-linux-hardened-v1-disposable-native-x86_64.contract.json \
  --expected-host-config docs/evidence/2026-07-19-m4-linux-hardened-v1-disposable-native-x86_64.host-config.json \
  --expected-revision 5455f14ecd7552277d6bd67e4a3f613acac7e2c5 \
  --expected-runtime-artifact tm-server@sha256:250c7ba11e4ba1a6f8ca6d622cd168220845e43dece7ae34470ecd6e6fdd7452
```

An additional disposable attempt used Docker Desktop on the arm64 development host with
`--platform linux/amd64`. The complete Rust test binary compiled and entered the focused canary,
but bubblewrap failed at the first sandbox launch with
`prctl(PR_SET_SECCOMP) reported EINVAL`. The native aarch64 canary on the same Docker Linux kernel
had already installed its architecture-matching filter successfully; the cross-architecture result
therefore did not qualify as an x86_64 kernel/seccomp execution gate and remains recorded as a failed
emulation attempt, not a product regression. The later native homolab run closes that architecture gate.

## Read-only lumo deployment audit

The 2026-07-18 lumo deployment-config snapshot remains a safe fail-closed deployment, but it is not an M4
deployment target yet:

- it pins pre-M4 source revision `bb8689ec593b93037b6cd1d76a638017647fbaf8`;
- its runtime image contains `tm-server`, but no bubblewrap isolation runtime or host config;
- its OpenRC environment does not set `TM_HOST_CONFIG`;
- its Podman runner mounts only application data and the PostgreSQL socket, and does not expose a
  linked project or an exclusive writable cgroup-v2 subtree; and
- its container-wide 512-pid limit is defense in depth, not per-run delegation. The lumo modules
  also declare no KVM, Firecracker, cloud-hypervisor, QEMU, Incus, or libvirt microVM target.

A 2026-07-19 live, read-only SSH preflight confirmed the deployed state rather than relying only on
deployment-config. lumo runs Linux `6.18.35-0-rpi` on `aarch64`; `lumo-tempestmiku` was started from
`localhost/tempestmiku:bb8689ec593b`, and its Podman process belonged to
`/libpod_parent/libpod-…`. The host uses cgroup v2, but both `cgroup.controllers` and the root
`cgroup.subtree_control` exposed only `cpuset cpu io pids`. The required `memory` controller was
absent from `/proc/cgroups` as well. Therefore lumo cannot satisfy the fixed
`linux_hardened_v1` memory limit or the v1 acceptance preflight in its current kernel configuration;
the correct result is fail closed before any service delegation or workload run. The exact retained
snapshot is [`2026-07-19-m4-lumo-readonly-preflight.json`](2026-07-19-m4-lumo-readonly-preflight.json).

Consequently linked-folder and `proc.run` authority remain disabled there. Closing M4 on lumo would
require an explicit owner decision to expose a curated project to that service, a deployment-config/image
change, a kernel exposing the cgroup-v2 `memory` controller, selected measured limits,
target-specific OpenRC/Podman cgroup delegation, deployment of a revision containing this profile,
and the wrapper canary under UID 10001 inside the final runtime.
None of those production mutations were made during this repo-only audit.

## Claim boundary

This closes the repo-verifiable `linux_hardened_v1` implementation and preserves the disposable
aarch64 and native x86_64 canaries. It does not prove production service delegation, representative workload
sizing/headroom, a live report on the selected production architecture, or production
cgroup exclusivity. The profile's claim is limited to a hostile workload on a trusted host kernel.
Hostile host-kernel containment requires mandatory microVM isolation and remains unclaimed; any
selected microVM also needs separate evidence. A concrete production Linux target still needs a
separate production acceptance design that binds supervisor activation, exact service role and
environment, deployed binary/image, linked roots, and a unique delegated-root lease before a
production report class may be introduced. The workload-metrics, production-target,
production-exclusivity, and any selected microVM gates remain open.

## Read-only homolab native-x86 candidate audit

A 2026-07-19 read-only SSH preflight found a viable host for the then-open disposable native
architecture gate. homolab runs Linux `7.0.10` on `x86_64`, uses cgroup v2, and exposes
`cpuset cpu io memory hugetlb pids rdma misc dmem` at both the root controller and subtree-control
surfaces. Docker and bubblewrap are installed. This satisfies the static prerequisites for
`tools/m4-native-x86-disposable-canary.sh`. After explicit owner approval, the disposable canary
passed as documented above; homolab was not selected as the production service target. Docker
`29.5.1` is native `amd64`, uses cgroup v2 with the
`systemd` driver, and is available through non-interactive `sudo`; the preflight observed about
30.0 GB each of available memory and Docker-filesystem space. The checked-in canary used a
privileged disposable Docker container only under that explicit approval. The exact preflight
snapshot is
[`2026-07-19-m4-homolab-readonly-preflight.json`](2026-07-19-m4-homolab-readonly-preflight.json).

## 2026-07-19 production follow-up

The open deployment gates described above were subsequently closed for the owner-selected homolab
hostile-workload/trusted-host-kernel contract. The persistent service, native workload metrics,
cgroup exclusivity, retained report, and restart proof are recorded in
[`2026-07-19-m4-production-homolab.md`](2026-07-19-m4-production-homolab.md). The historical lumo and
disposable-canary conclusions remain valid; hostile-kernel containment and microVM isolation remain
outside the accepted claim.
