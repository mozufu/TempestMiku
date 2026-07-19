# M4 Linux `proc.run` isolation canary — 2026-07-18

## Result

The final-source gated Linux canary passed: `1 passed; 0 failed; 82 filtered out`. It exercised the real
`ProcIsolationConfig::LinuxBubblewrap` path through `proc.run`, not an argv-plan mock. The
machine-readable companion is
[`2026-07-18-m4-linux-proc-isolation.json`](2026-07-18-m4-linux-proc-isolation.json).

The test proved all of the following in one disposable privileged Linux container:

- root-owned static BusyBox applets invoked by direct argv vectors could read and write only through
  the granted linked-folder mount;
- `/etc/passwd` from the outer container was absent inside the sandbox;
- an outer-namespace loopback listener was reachable by the test process but not by BusyBox `wget`
  inside the unshared network namespace;
- an unlisted `TM_M4_AMBIENT_SECRET` variable was removed, and `PATH` was reduced exactly to the
  configured runtime root;
- `RLIMIT_NOFILE=64` was visible to the isolated child; and
- a missing required launcher returned `CapabilityDenied` before approval, never ran the command on
  the host, and never created the fallback marker;
- the bubblewrap launcher and approved executable were both held by descriptor through spawn; their
  configured launcher/runtime ancestry was root-owned and non-writable (apart from root-owned sticky
  directories such as `/tmp`);
- replacing the complete linked-root path after final validation but before bubblewrap mount setup
  still exposed the approved directory inode, not the replacement; and
- replacing only the approved cwd entry during the same window still entered the approved cwd
  inode, not the replacement inside the otherwise stable linked root; and
- replacing the approved executable path after final validation but before bubblewrap consumed its
  arguments still executed the approved descriptor-mounted inode, not the replacement.

## Environment and command

- Docker/OrbStack, privileged disposable container
- `rust:1.88-trixie`, image ID
  `sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db`
- Linux `7.0.5-orbstack-00330-ge3df4e19b0a0-dirty`, `aarch64`
- Rust/Cargo `1.88.0`
- bubblewrap `0.11.0-2+deb13u1`, root-owned mode `0755`
- BusyBox static `1:1.37.0-6+b8`, with direct applet symlinks installed under the root-owned
  explicit runtime root

The successful test command was:

```sh
docker run --rm --privileged \
  -v "$PWD":/workspace:ro \
  -w /workspace \
  -e PATH=/opt/tempestmiku-isolation-runtime/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  -e CARGO_TARGET_DIR=/tmp/tm-m4-executable-fd-target \
  -e TM_LINUX_ISOLATION_TESTS=1 \
  -e TM_M4_AMBIENT_SECRET=must-not-cross \
  rust:1.88-trixie \
  sh -lc '
    apt-get update -qq &&
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq bubblewrap busybox-static &&
    install -d -m 0755 /opt/tempestmiku-isolation-runtime/bin &&
    install -m 0755 "$(command -v bwrap)" /opt/tempestmiku-isolation-runtime/bin/bwrap &&
    install -m 0755 "$(command -v busybox)" /opt/tempestmiku-isolation-runtime/bin/busybox &&
    for name in cat env test touch wget; do
      ln -s busybox "/opt/tempestmiku-isolation-runtime/bin/$name"
    done &&
    cargo test -p tm-host \
      linked::tests::linux_isolation::gated_linux_bubblewrap_proc_run_enforces_profile \
      -- --exact --test-threads=1 --nocapture'
```

Normal `cargo test` runs skip the gated body, remain network-free, and require neither Linux nor
bubblewrap.

## Findings fixed during the canary

The first disposable image used bubblewrap `0.8.0`. It rejected `--argv0` and failed closed; it did
not fall back to host execution. The validated deployment version is `0.11.0`. A second run then
showed that `--disable-userns` requires an explicit `--unshare-user`; `--unshare-all` alone does not
create that namespace. The profile now adds `--unshare-user`, and its argv regression test pins the
requirement.

Independent audits then found path lookup windows after final descriptor-relative validation. The
profile now hands the already-open linked-root, cwd, and executable descriptors to bubblewrap's
`--bind-fd`/`--ro-bind-fd` operations, clearing `FD_CLOEXEC` only in the forked child. The launcher is
also descriptor-pinned and child-side identity-checked, and launcher/runtime roots validate their
complete canonical ancestry. The delayed-launcher regression deterministically replaces the linked
root, cwd, and executable paths after spawn and before bubblewrap consumes them; every case retained
the approved inode.

## Claim boundary

This closes the executable namespace/rlimit canary for the implemented M4 profile. It proves
bubblewrap mount, user, network, and PID/IPC/UTS namespace construction, capability dropping,
environment filtering, and the observed `RLIMIT_NOFILE` ceiling in the disposable Linux
environment. It also proves descriptor-pinned launcher, linked-root, cwd, and executable identity
across concurrent rename/replacement in that environment. `RLIMIT_AS` and `RLIMIT_NPROC` were
configured by the same child hook but were not independently exhausted by this canary. This
descriptor-execution claim is Linux-specific; non-Linux Unix hosts retain the pre-existing final
path identity check and are not covered by this bubblewrap evidence.

It does **not** claim a seccomp syscall policy, cgroup accounting/enforcement, microVM isolation,
hostile-kernel containment, or a production-host Linux canary. The later
[`linux_hardened_v1` evidence](2026-07-18-m4-linux-hardened-v1.md) closes fixed-seccomp and per-run
cgroup-v2 software/disposable-Linux gates; those stronger results still must not be inferred from
this lower profile, and neither document substitutes for production-host or microVM acceptance.
