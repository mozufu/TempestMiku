# M4 coordinator/worker production deployment

Date: 2026-07-19 (Asia/Taipei)

Status: **PASS** for the owner-selected one-coordinator/one-worker deployment and the existing
hostile-workload/trusted-host-kernel M4 boundary.

## Deployed topology

- `lumo` runs the only authoritative `tm-server`. It owns sessions, model/persona state, durable
  turns, approvals, dreams, cron, memory, artifacts, and client APIs.
- `homolab` runs one `tm-worker`, not a second server. It exposes only the configured linked
  `fs.*`, `code.search`, `proc.run`, and `linked://` boundaries.
- Requests cross the Tailnet to `100.110.95.111:18787` using the versioned v1 JSON contract and a
  shared HMAC-SHA256 key. Timestamp, nonce, method, path, and exact body are signed; worker nonce
  replay is rejected.
- The relationship is asymmetric coordinator/worker, not peer-to-peer and not master/slave. The
  worker has no model, client API, persona, memory authority, or approval authority.

## Bound revisions

- TempestMiku feature revision: `f75c744dd29ab6c28bb626613ce97b3b12b9b306`.
- Final deployed worker module/checkout revision: `14ba767ca6ed93363668c2220f7c5440bc555561`.
- Lumo coordinator image source revision: `2cffd0e4e86e2ec669963cc986ed0783cd4db80d`.
  Later TempestMiku commits through `14ba767` change only the NixOS worker module, not the
  coordinator or worker Rust binaries.
- Deployment configuration revision: `4774c08`.
- Homolab generation:
  `/nix/store/4p0jzs1bdm69v35rj974a8bjl8waa8vy-nixos-system-homolab-26.05.20260717.293d6ab`.

## Automated gates

Before deployment, `cargo test --workspace --all-features --locked --no-fail-fast` passed across
the workspace, including 328 `tm-server` tests, 79 `tm-host` tests, two `tm-worker` tests, and three
`tm-worker-protocol` tests. `cargo fmt --all --check` and strict all-target/all-feature Clippy with
`-D warnings` also passed. The deployment flake check, explicit homolab NixOS evaluation, lumo Home
Manager evaluation, Nix formatting, and diff checks passed.

The coordinator integration tests cover lumo-owned approval brokering, exact action-digest
resolution, cancellation forwarding, artifact localization with digest/size verification, session
scope propagation, remote `linked://`, and refusal to configure local and remote linked hosts
together.

## Live production canaries

Homolab reported:

```text
protocolVersion=1 workerId=homolab-m4 ready=true
ActiveState=active SubState=running User=tempestmiku-worker Group=tempestmiku-worker
Delegate=yes root_procs=0 controllers="cpu memory pids" service_procs=1
checkout=14ba767ca6ed93363668c2220f7c5440bc555561
```

A lumo-side signed transport canary used unique durable job IDs and observed:

```json
{"workerId":"homolab-m4","fsRead":"succeeded","idempotentJobId":"succeeded","procRun":"succeeded","approvalResolvedFrom":"lumo"}
```

The read returned the real operator-provisioned TempestMiku checkout. Re-submitting its exact job
ID returned the retained terminal result rather than executing again. The `proc.run` job paused in
`awaiting_approval`, bound the approval to the worker-provided action digest, received the signed
resolution from lumo, and completed inside the production bubblewrap/seccomp/cgroup profile with
exit code zero.

For the no-fallback canary, the worker service was stopped temporarily. Its Tailnet endpoint became
unreachable while lumo's coordinator health remained good. The coordinator container exposed
`TM_REMOTE_WORKER_CONFIG` and no `TM_HOST_CONFIG`; the worker was then restored and returned active.

Lumo's OpenRC service and all normal lumo smoke checks passed. The running coordinator container is
`localhost/tempestmiku:2cffd0e4e86e`, mounts the remote-worker config and signing credential, and
does not contain a local linked-host configuration. The Mac `darwin-rebuild switch --flake
.#m3air` also completed successfully at deployment configuration revision `4774c08`.

## Production integration findings

1. The linked checkout is explicitly operator-provisioned. The module creates the state and alias
   roots, but it does not clone repositories or silently substitute another checkout.
2. `systemd-tmpfiles` rejects an unsafe transition from the worker-owned state root to a root-owned
   child. Aligning the linked-root owner with the dedicated worker user preserves the closed state
   directory and allows deterministic alias creation.
3. The service PATH must put the immutable isolation runtime first. Otherwise NixOS resolves
   allowlisted command names to normal system packages, which the runtime-root policy correctly
   rejects.
4. `ProtectKernelTunables`, `ProtectKernelLogs`, and, in combination with the remaining service
   hardening, `ProtectHostname`, construct outer namespace or `/proc` mounts that prevent nested
   bubblewrap from mounting its private `/proc`. Transient-unit isolation tests identified the
   exact conflict. These three flags are therefore omitted; `NoNewPrivileges`, empty capability
   sets, private devices/tmp, protected home/system/modules/clock, strict read-only system paths,
   the dedicated UID/GID, seccomp, descriptor-pinned mounts, and delegated per-run cgroups remain.

## Claim boundary

This proves one authoritative coordinator and one bounded remote worker on the current lumo and
homolab production hosts. It does not claim protection against a compromised homolab kernel,
cross-owner multi-tenancy, peer worker coordination, automatic repository provisioning, or a local
execution fallback.
