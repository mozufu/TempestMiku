# M4 production acceptance — homolab x86_64

Date: 2026-07-19 (Asia/Taipei)

Status: **PASS** for the selected M4 threat boundary: a hostile `proc.run` workload on the
owner-controlled homolab host with a trusted host kernel. Host-kernel compromise containment and
microVM isolation are intentionally not claimed.

Machine-readable report:
[`2026-07-19-m4-production-homolab-x86_64.json`](2026-07-19-m4-production-homolab-x86_64.json),
SHA-256 `6e79ecef4ab65b69bd5c2af061ed71f514726b8255c32939f7da37a7525e90d0`.

## Selected target and persistent deployment

- Target: `homolab`, native `x86_64`, NixOS `26.05.20260528.ec942ba`, Linux `7.0.10`.
- Configuration source: the owner's selected `homolab` Nix flake target, not `/etc/nixos`.
- Deployment configuration base revision: `693fc9d402dba11ca5d711c6f2c24324ef8b3e1d`; the scoped
  M4 configuration was staged but not committed during this acceptance run.
- The changed deployment configuration covered the flake inputs, host AI service aggregation, and
  the new TempestMiku M4 service module. Its staged diff check passed.
- The server and runtime flake inputs use the content-addressed `*-source` paths already retained by
  the active system closure, rather than temporary build-result symlinks. Rebuilding after this
  change reproduced the exact same system generation.
- `nixos-rebuild build`, `test`, and `switch` all selected
  `/nix/store/cnlkzs0c0l5fyhi55kw8if97za64flxj-nixos-system-homolab-26.05.20260528.ec942ba`.
  Both `/run/current-system` and `/nix/var/nix/profiles/system` resolve to it after the switch.

The service uses a dedicated UID/GID `23017`, starts on loopback as the API role, and delegates
exactly `cpu memory pids`. `DelegateSubgroup=service` puts the long-lived server below the delegated
root, leaving the unit root free for per-run cgroups. The launch wrapper verifies that topology and
enables all three controllers before `exec` of `tm-server`.

## Bound identities

- TempestMiku source revision: `6822077058994d10a796f3d2cffa369e01a05108`.
- Source tree: `4b0ff81ca0f59453203a12afa8556275a80774d2`.
- Server binary SHA-256: `d77d55eb3734ca128c6afcb0d70207ee3da9204cc9b52223fda56016fb65ea75`.
- Root-owned test binary SHA-256:
  `e288cf7f012c35ba0f90253ccc8d8c266eb47902a3ca319bd1dcd2d81a6ea440`.
- Production host-config SHA-256:
  `e40918af2313f17960b4a22606c6c1a12e853a5abfbf17025d27acf43c58e5c3`.
- systemd fragment SHA-256:
  `3d3aecb084e9e19e88c5df8a52ebd4b15af5bb7c1289e43117ef09b8fbb5d806`.

## Production workload canary

The acceptance runner started the real `tm-host` Linux test under service UID/GID in an
acceptance-only child cgroup. The test then exercised the production `linux_hardened_v1` path:
root-owned immutable bubblewrap/runtime roots, the fixed architecture-specific seccomp policy, and
one unpredictable cgroup leaf per `proc.run`. It observed:

| Scenario | Required | Observed peak |
|---|---:|---:|
| resident memory | 256 MiB | 269,959,168 bytes |
| child processes | 16 children | 19 pids |
| busy CPU | 750 ms | 753,204 µs |

The configured limits remain 1 GiB memory, zero swap, 64 pids, and one CPU, giving at least 4×
memory and 3.76× observed-pid headroom. The runner sampled 865 times, verified the exact leaf limits,
and observed no per-run cgroup residue. Before and after the run, the delegated root contained only
the `service` child and every captured systemd property was byte-stable.

## Restart and exposure proof

After the persistent switch, an explicit service restart changed PID `72299` to `73103`. The new
process remained `active/running`, `enabled`, and `NRestarts=0`; its executable hash matched the
accepted server. The unit root had no resident process, the only child cgroup was `service`, and the
server listened only on `127.0.0.1:8787`.

## Failures resolved during production integration

1. `/etc/nixos` was rejected as the deployment source because it built a different, stale system;
   the selected `homolab` flake target was the authoritative deployment configuration.
2. A pre-start controller setup was invalid with `DelegateSubgroup`: systemd places only the main
   service process in the delegated subgroup. A single main-process launch wrapper now verifies the
   subgroup, enables the controllers, and then `exec`s the server.
3. The original cgroup validator incorrectly required the delegated root's `cgroup.kill` to be
   writable. systemd intentionally retains root ownership there while delegating the child leaf.
   The validator now verifies writable leaf authority, which matches systemd's production contract.
4. The acceptance script's `/usr/bin/env` shebang is unavailable on NixOS. Invoking it through the
   NixOS Python path produced the retained report without changing its validation logic.

## Claim boundary

This closes M4 for the owner-selected, trusted-host-kernel production contract. It does not claim
containment when the homolab kernel itself is hostile or compromised. A future deployment that puts
the host kernel in scope must add and separately accept a microVM; that optional higher-assurance
deployment does not reopen this selected M4 contract. This evidence also does not close P6.6's
physical Android speech-quality and lifecycle/resource matrix.
