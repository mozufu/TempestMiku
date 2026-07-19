# Deploy one coordinator and one remote worker

This guide describes the portable TempestMiku deployment contract. It does not require Nix, SOPS,
Tailscale, systemd, a particular Linux distribution, or the project owner's host names.

The topology is asymmetric:

```text
clients -> tm-server coordinator -> signed jobs -> tm-worker
```

- Run exactly one authoritative `tm-server`. It owns clients, sessions, models, persona, memory,
  approvals, durable turns, schedules, and coordinator artifacts.
- Run one `tm-worker` for the protocol-v1 linked-host boundary. It owns no model, memory, client API,
  or approval policy. It executes only the capabilities and linked aliases sent by the coordinator.
- The worker and coordinator do not elect a leader and are not peers or master/slave replicas.
- Protocol v1 has no local-execution fallback and no multi-worker load balancing.

## 1. Requirements

Coordinator requirements are the same as a normal `tm-server` deployment; see
[`running-miku.md`](running-miku.md). Adding a remote worker additionally requires:

- network reachability from the coordinator to the worker;
- synchronized clocks within 30 seconds;
- one shared 32-byte HMAC key, encoded as exactly 64 lowercase hexadecimal characters;
- a secret delivery mechanism that materializes that key as a file on both processes;
- persistent worker job-ledger and artifact directories;
- an operator-provisioned linked directory or checkout;
- a dedicated, unprivileged worker identity with access only to those paths.

The secret file can come from Vault Agent, Kubernetes Secrets, Docker/Podman secrets, systemd
credentials, a cloud secret manager sidecar, an encrypted configuration tool, or a manually
provisioned root-readable file. TempestMiku does not require a particular secret manager. Do not put
the key directly in either JSON config.

## 2. Build without Nix

Use a stable Rust toolchain with Rust 2024 edition support:

```sh
git clone https://github.com/mozufu/TempestMiku.git
cd TempestMiku
git checkout --detach '<reviewed revision>'
cargo build --locked --release --package tm-server --package tm-worker
```

The binaries are `target/release/tm-server` and `target/release/tm-worker`. Install them through the
packaging mechanism used by your operating system or container platform. Build and deploy an exact
reviewed revision; do not make a production worker follow a moving branch implicitly.

Nix users may instead consume the `tmServer`, `tmWorker`, and `m4IsolationRuntime` flake packages.

## 3. Choose the network transport

Every job request is authenticated with HMAC-SHA256 over the method, path, timestamp, nonce, and
exact body digest. HMAC authenticates requests but does not encrypt traffic.

The coordinator accepts these endpoint forms:

- `http://127.0.0.1:<port>` or `http://[::1]:<port>` for same-host proxying;
- plain HTTP to a Tailnet IPv4 address in `100.64.0.0/10`;
- `https://<worker-origin>` for every other network.

Private RFC1918 addresses, Kubernetes service names, WireGuard addresses outside the Tailnet range,
and public host names still require HTTPS. Put a TLS reverse proxy or service mesh in front of the
worker, keep the worker listener private, and make the coordinator trust the issuing CA. The
configured endpoint must be an origin only: no path prefix, query, or fragment. Forward `/v1/...`
paths and request bodies unchanged, because the signature binds both.

Restrict worker ingress to the coordinator at the firewall, security-group, network-policy, VPN, or
service-mesh layer. The unsigned health route identifies readiness but does not grant execution.

## 4. Provision the HMAC key

Generate 32 cryptographically random bytes once, encode them as lowercase hexadecimal, store the
value in your secret manager, and mount or render the same value into two files:

```text
coordinator: /run/secrets/tempestmiku-worker-key
worker:      /run/secrets/tempestmiku-worker-key
```

Both files should be readable only by their service identity. Validate the format without printing
the value. Protocol v1 accepts one key at a time, so rotation is coordinated rather than gradual:
quiesce remote jobs, update both secret files, restart both sides, then run the signed canary.

## 5. Configure the worker

Create a persistent ledger root and a linked root owned by the worker identity. Repository cloning,
syncing, and revision selection remain operator responsibilities; `tm-worker` never provisions a
checkout.

Start with read-only file and code access and no executable commands. For example,
`/etc/tempestmiku/worker-host.json`:

```json
{
  "linked_folders": [
    {
      "name": "project",
      "path": "/srv/tempestmiku-linked/project",
      "mode": "ro",
      "commands": [],
      "safe_args": []
    }
  ],
  "approvals": {
    "mode": "deny",
    "timeout_ms": 60000
  },
  "artifact_root": "/var/lib/tempestmiku-worker/artifacts",
  "proc_run_timeout_ms": 180000,
  "proc_isolation": {
    "provider": "disabled"
  }
}
```

Set `mode` to `rw` only when remote mutation is intended. Alias names must be unique lowercase
identifiers. Keep `commands` empty until a production isolation profile is configured and accepted.

Create `/etc/tempestmiku/worker.json`:

```json
{
  "workerId": "worker-1",
  "listenAddr": "127.0.0.1:18787",
  "signingKeyFile": "/run/secrets/tempestmiku-worker-key",
  "hostConfigFile": "/etc/tempestmiku/worker-host.json",
  "ledgerRoot": "/var/lib/tempestmiku-worker",
  "approvalTimeoutMs": 60000,
  "maxConcurrentJobs": 4,
  "maxConcurrentProcRuns": 1,
  "retentionSeconds": 86400
}
```

Use a non-loopback `listenAddr` only when the network boundary is intentional. The TLS endpoint may
be a reverse proxy in front of this plain HTTP listener.

Run the worker under any suitable supervisor:

```sh
TM_WORKER_CONFIG=/etc/tempestmiku/worker.json \
RUST_LOG=info \
/usr/local/bin/tm-worker
```

systemd, OpenRC, runit, s6, Docker/Podman, Kubernetes, Nomad, and other supervisors are all valid if
they preserve the same filesystem ownership, secret-file, restart, and network boundaries. Use one
active worker process per ledger root. A restart marks interrupted nonterminal jobs indeterminate;
it does not replay an uncertain mutation.

## 6. Configure the coordinator

Create `/etc/tempestmiku/remote-worker.json`:

```json
{
  "workerId": "worker-1",
  "endpoint": "https://worker.example.internal",
  "signingKeyFile": "/run/secrets/tempestmiku-worker-key",
  "linkedAliases": ["project"]
}
```

The `workerId` and aliases must exactly match the worker-side configuration. Set this environment
variable on the existing coordinator service:

```sh
TM_REMOTE_WORKER_CONFIG=/etc/tempestmiku/remote-worker.json
```

`TM_HOST_CONFIG` may still configure non-linked facilities such as egress, but its
`linked_folders` list must be empty. Startup rejects simultaneous local and remote linked-folder
hosts in protocol v1. `TM_SERVER_ROLE=all` refers only to the coordinator's internal API and durable
turn/dream/cron supervision; it does not create another external worker.

## 7. Optional `proc.run` isolation

File and code operations work without process execution. Production `proc.run` is opt-in and
currently requires Linux isolation:

- `linux_bubblewrap` requires an explicit bubblewrap launcher, immutable runtime roots containing
  the allowlisted executables, and bounded address-space/process/file limits.
- `linux_hardened_v1` additionally requires the fixed seccomp profile and a delegated cgroup-v2
  subtree with `cpu`, `memory`, and `pids` controllers.

An unavailable or invalid isolation profile fails closed; it never falls back to direct host
execution. On non-Linux workers, keep `commands` empty and `proc_isolation` disabled. Do not copy
runtime-root or cgroup paths from another machine: bind them to the packages, service manager, and
kernel actually deployed, then run the M4 acceptance canary described in
[`running-miku.md`](running-miku.md).

## 8. Rollout order

1. Build and verify both binaries at reviewed revisions.
2. Provision the key, persistent directories, service identity, and exact linked checkout.
3. Deploy the worker first and verify its private health endpoint.
4. Deploy the coordinator with `TM_REMOTE_WORKER_CONFIG` and no local linked folders.
5. Run signed end-to-end acceptance before granting real project turns.
6. Update clients only after the coordinator and worker pass.

Keep the protocol version compatible during rolling changes. If a future release changes the wire
contract incompatibly, use a maintenance window rather than pointing mismatched processes at one
another.

## 9. Acceptance checklist

`GET /v1/health` is necessary but insufficient. Verify all applicable items:

- health reports protocol version `1`, the expected worker ID, and `ready: true`;
- invalid HMAC, stale timestamps, and reused nonces are rejected;
- `fs.read` returns content from the intended linked root and cannot escape it;
- submitting the same durable job ID and exact request returns the retained result without running
  it again;
- approval-gated operations pause, and only the coordinator resolves the exact action digest;
- worker artifacts are copied into the coordinator artifact store with size and SHA-256 checks;
- cancellation is forwarded and bounded;
- a worker restart retains terminal jobs and marks uncertain in-flight jobs indeterminate;
- worker loss produces an explicit remote-unavailable error while the coordinator remains healthy;
- no local linked execution occurs during worker loss;
- if `proc.run` is enabled, the real bubblewrap/seccomp/cgroup canary passes and leaves no cgroup
  residue.

## 10. Scaling and recovery limits

Protocol v1 intentionally supports one configured remote worker. Do not put multiple independent
workers behind a round-robin load balancer: the durable ledger, nonce cache, approval waiter, and
artifacts are worker-local. High availability, worker discovery, shared ledgers, and worker-to-worker
communication are not implemented.

Back up the coordinator database/artifacts and the worker ledger/artifacts according to their normal
durability requirements. The linked checkout is operator-managed source state, not a substitute for
the canonical Git remote or project backup.
