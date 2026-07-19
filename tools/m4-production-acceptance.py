#!/usr/bin/env python3
"""Retain fail-closed evidence for the selected M4 production systemd target."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import pwd
import grp
import shutil
import stat
import subprocess
import sys
import time
from typing import Any


UNIT = "tempestmiku-m4.service"
EXPECTED_UID = 23017
EXPECTED_GID = 23017
REQUIRED_CONTROLLERS = {"cpu", "memory", "pids"}
TEST_NAME = (
    "linked::tests::linux_isolation::"
    "gated_linux_hardened_v1_enforces_seccomp_and_cgroup_lifecycle"
)


class AcceptanceError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise AcceptanceError(message)


def run(argv: list[str], *, cwd: Path | None = None) -> str:
    try:
        result = subprocess.run(
            argv,
            cwd=cwd,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        fail(f"command failed: {argv!r}: {detail.strip()}")
    return result.stdout.strip()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def file_identity(path: Path, *, executable: bool = False) -> dict[str, Any]:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        fail(f"{path} must be a regular non-symlink file")
    mode = stat.S_IMODE(metadata.st_mode)
    if metadata.st_uid != 0 or mode & 0o022:
        fail(f"{path} must be root-owned and not group/world writable")
    if executable and mode & 0o111 == 0:
        fail(f"{path} must be executable")
    return {
        "path": str(path),
        "canonicalPath": str(path.resolve(strict=True)),
        "sha256": sha256_file(path),
        "sizeBytes": metadata.st_size,
        "uid": metadata.st_uid,
        "gid": metadata.st_gid,
        "mode": mode,
        "device": metadata.st_dev,
        "inode": metadata.st_ino,
    }


def read_int(path: Path) -> int:
    return int(path.read_text(encoding="utf-8").strip())


def systemctl_show(properties: list[str]) -> dict[str, str]:
    output = run(
        ["systemctl", "show", UNIT, *[f"--property={name}" for name in properties]]
    )
    values: dict[str, str] = {}
    for line in output.splitlines():
        key, separator, value = line.partition("=")
        if not separator or key in values:
            fail(f"unexpected systemctl show output: {line!r}")
        values[key] = value
    if set(values) != set(properties):
        fail(f"systemctl omitted properties: {sorted(set(properties) - set(values))}")
    return values


def exact_words(value: str) -> set[str]:
    return {word for word in value.replace(",", " ").split() if word}


def source_identity(repo: Path) -> dict[str, Any]:
    if run(["git", "status", "--porcelain=v1", "--untracked-files=all"], cwd=repo):
        fail("production evidence requires a clean source tree")
    revision = run(["git", "rev-parse", "HEAD"], cwd=repo)
    if len(revision) != 40:
        fail("source revision is not a full Git object id")
    return {
        "revision": revision,
        "tree": run(["git", "rev-parse", "HEAD^{tree}"], cwd=repo),
        "remote": run(["git", "remote", "get-url", "origin"], cwd=repo),
    }


def validate_host_config(path: Path, cgroup_root: Path) -> tuple[dict[str, Any], Path]:
    identity = file_identity(path)
    config = json.loads(path.read_text(encoding="utf-8"))
    isolation = config.get("proc_isolation")
    if not isinstance(isolation, dict) or isolation.get("provider") != "linux_hardened_v1":
        fail("host config must select linux_hardened_v1")
    if isolation.get("cgroup_root") != str(cgroup_root):
        fail("host config cgroup_root does not match the selected systemd unit")
    limits = isolation.get("cgroup_limits")
    expected_limits = {
        "memory_max_bytes": 1073741824,
        "memory_swap_max_bytes": 0,
        "pids_max": 64,
        "cpu_quota_micros": 100000,
        "cpu_period_micros": 100000,
    }
    if limits != expected_limits:
        fail("host config must use the reviewed production cgroup limits")
    roots = isolation.get("runtime_roots")
    if not isinstance(roots, list) or len(roots) != 1:
        fail("host config must bind one immutable isolation runtime")
    runtime = Path(roots[0])
    launcher = Path(isolation.get("launcher", ""))
    if launcher != runtime / "bin/bwrap":
        fail("host config launcher must be the selected runtime bwrap")
    for command in ("bwrap", "busybox", "thread-probe", "resource-probe"):
        file_identity(runtime / "bin" / command, executable=True)
    return {"identity": identity, "config": config}, runtime


def inspect_leaf(leaf: Path, metrics: dict[str, int]) -> None:
    limits = {
        "memory.max": "1073741824",
        "memory.swap.max": "0",
        "pids.max": "64",
        "cpu.max": "100000 100000",
    }
    for name, expected in limits.items():
        if leaf.joinpath(name).read_text(encoding="utf-8").strip() != expected:
            fail(f"{leaf.name} has an unexpected {name}")
    metrics["maxMemoryCurrentBytes"] = max(
        metrics["maxMemoryCurrentBytes"], read_int(leaf / "memory.current")
    )
    metrics["maxPidsCurrent"] = max(
        metrics["maxPidsCurrent"], read_int(leaf / "pids.current")
    )
    cpu = {
        line.split()[0]: int(line.split()[1])
        for line in (leaf / "cpu.stat").read_text(encoding="utf-8").splitlines()
        if len(line.split()) == 2 and line.split()[1].isdigit()
    }
    metrics["maxCpuUsageUsec"] = max(
        metrics["maxCpuUsageUsec"], cpu.get("usage_usec", 0)
    )
    metrics["observations"] += 1


def launch_canary(
    test_binary: Path,
    host_config: Path,
    runtime: Path,
    cgroup_root: Path,
    acceptance_root: Path,
) -> tuple[dict[str, int], str]:
    acceptance_cgroup = cgroup_root / "acceptance-v1"
    if acceptance_cgroup.exists():
        fail("stale acceptance-v1 cgroup exists")
    acceptance_cgroup.mkdir()
    os.chown(acceptance_cgroup, EXPECTED_UID, EXPECTED_GID)
    environment = {
        "HOME": str(acceptance_root),
        "PATH": f"{runtime}/bin:/run/current-system/sw/bin",
        "RUST_BACKTRACE": "1",
        "TMPDIR": str(acceptance_root),
        "TM_HOST_CONFIG": str(host_config),
        "TM_LINUX_HARDENED_TESTS": "1",
    }

    def prepare_child() -> None:
        (acceptance_cgroup / "cgroup.procs").write_text("0\n", encoding="ascii")
        os.setgroups([])
        os.setgid(EXPECTED_GID)
        os.setuid(EXPECTED_UID)

    process = subprocess.Popen(
        [
            str(test_binary),
            TEST_NAME,
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ],
        cwd=acceptance_root,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        preexec_fn=prepare_child,
    )
    metrics = {
        "maxMemoryCurrentBytes": 0,
        "maxPidsCurrent": 0,
        "maxCpuUsageUsec": 0,
        "observations": 0,
    }
    deadline = time.monotonic() + 90
    try:
        while process.poll() is None:
            if time.monotonic() > deadline:
                process.kill()
                fail("production canary exceeded 90 seconds")
            for leaf in sorted(cgroup_root.glob("tm-run-v1-*")):
                inspect_leaf(leaf, metrics)
            time.sleep(0.005)
        output = process.communicate(timeout=5)[0]
        if process.returncode != 0:
            fail(f"production canary failed ({process.returncode}):\n{output[-4000:]}")
        if "test result: ok. 1 passed" not in output:
            fail("production canary did not report exactly one passing test")
        if list(cgroup_root.glob("tm-run-v1-*")):
            fail("per-run cgroup residue remained after the production canary")
        if metrics["maxMemoryCurrentBytes"] < 256 * 1024 * 1024:
            fail("the representative 256 MiB workload was not observed")
        if metrics["maxPidsCurrent"] < 17:
            fail("the representative 16-child workload was not observed")
        if metrics["maxCpuUsageUsec"] < 100_000:
            fail("the representative CPU workload was not observed")
        return metrics, output
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        for _ in range(100):
            try:
                acceptance_cgroup.rmdir()
                break
            except OSError:
                time.sleep(0.01)
        if acceptance_cgroup.exists():
            fail("acceptance cgroup could not be removed")


def accept(args: argparse.Namespace) -> dict[str, Any]:
    if platform.system() != "Linux" or os.geteuid() != 0:
        fail("production acceptance must run as root on Linux")
    if platform.node() != args.expected_hostname:
        fail(f"expected hostname {args.expected_hostname!r}, got {platform.node()!r}")
    if platform.machine() != "x86_64":
        fail("the selected production target must be native x86_64")
    if pwd.getpwnam("tempestmiku-m4").pw_uid != EXPECTED_UID:
        fail("tempestmiku-m4 user does not have the reviewed UID")
    if grp.getgrnam("tempestmiku-m4").gr_gid != EXPECTED_GID:
        fail("tempestmiku-m4 group does not have the reviewed GID")

    properties = [
        "ActiveState",
        "SubState",
        "UnitFileState",
        "MainPID",
        "User",
        "Group",
        "Delegate",
        "DelegateControllers",
        "DelegateSubgroup",
        "FragmentPath",
        "NRestarts",
        "Environment",
        "ExecStart",
    ]
    before = systemctl_show(properties)
    if before["ActiveState"] != "active" or before["SubState"] != "running":
        fail("selected production service is not active/running")
    if before["UnitFileState"] != "enabled":
        fail("selected production service is not persistently enabled")
    if before["User"] != "tempestmiku-m4" or before["Group"] != "tempestmiku-m4":
        fail("selected production service has unexpected credentials")
    if before["Delegate"] != "yes":
        fail("selected production service is not a delegated cgroup owner")
    if exact_words(before["DelegateControllers"]) != REQUIRED_CONTROLLERS:
        fail("selected production service must delegate exactly cpu, memory, and pids")
    if before["DelegateSubgroup"] != "service":
        fail("selected production service must use DelegateSubgroup=service")
    main_pid = int(before["MainPID"])
    if main_pid <= 1:
        fail("selected production service has no stable MainPID")

    cgroup_root = Path("/sys/fs/cgroup/system.slice") / UNIT
    if not cgroup_root.is_dir():
        fail("selected production cgroup root is absent")
    relative_cgroup = next(
        (
            line.removeprefix("0::")
            for line in Path(f"/proc/{main_pid}/cgroup").read_text().splitlines()
            if line.startswith("0::")
        ),
        "",
    )
    if relative_cgroup != f"/system.slice/{UNIT}/service":
        fail("tm-server is not isolated in the delegated service subgroup")
    if (cgroup_root / "cgroup.procs").read_text().strip():
        fail("delegated unit root must not contain resident processes")
    if not REQUIRED_CONTROLLERS.issubset(
        exact_words((cgroup_root / "cgroup.subtree_control").read_text())
    ):
        fail("delegated unit root does not activate cpu, memory, and pids")
    initial_children = {path.name for path in cgroup_root.iterdir() if path.is_dir()}
    if initial_children != {"service"}:
        fail(f"production cgroup exclusivity is not clean: {sorted(initial_children)}")

    environment = before["Environment"].split()
    host_config_values = [
        item.removeprefix("TM_HOST_CONFIG=")
        for item in environment
        if item.startswith("TM_HOST_CONFIG=")
    ]
    if len(host_config_values) != 1:
        fail("service must expose exactly one TM_HOST_CONFIG")
    host_config_path = Path(host_config_values[0])
    host, runtime = validate_host_config(host_config_path, cgroup_root)
    server_binary = Path(f"/proc/{main_pid}/exe").resolve(strict=True)
    server = file_identity(server_binary, executable=True)
    test_binary = Path(args.test_binary)
    test_identity = file_identity(test_binary, executable=True)
    source = source_identity(Path(args.repo))

    acceptance_root = Path("/var/lib/tempestmiku-m4/acceptance")
    acceptance_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chown(acceptance_root, EXPECTED_UID, EXPECTED_GID)
    for child in acceptance_root.iterdir():
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()
    metrics, _output = launch_canary(
        test_binary, host_config_path, runtime, cgroup_root, acceptance_root
    )

    after = systemctl_show(properties)
    for name in properties:
        if after[name] != before[name]:
            fail(f"service property {name} changed during acceptance")
    final_children = {path.name for path in cgroup_root.iterdir() if path.is_dir()}
    if final_children != {"service"}:
        fail(f"production cgroup residue remained: {sorted(final_children)}")

    machine_id = Path("/etc/machine-id").read_bytes().strip()
    system_generation = Path("/run/current-system").resolve(strict=True)
    return {
        "schemaVersion": 1,
        "reportType": "tempestmiku.m4.production_service.acceptance",
        "status": "passed",
        "evidenceClass": "production_service",
        "generatedAtUnixMs": int(time.time() * 1000),
        "target": {
            "hostname": platform.node(),
            "architecture": platform.machine(),
            "kernelRelease": platform.release(),
            "nixosVersion": run(["nixos-version"]),
            "machineIdSha256": hashlib.sha256(machine_id).hexdigest(),
            "systemGeneration": str(system_generation),
        },
        "source": source,
        "service": {
            "unit": UNIT,
            "uid": EXPECTED_UID,
            "gid": EXPECTED_GID,
            "mainPid": main_pid,
            "properties": before,
            "serverBinary": server,
            "hostConfig": host,
            "fragment": file_identity(
                Path(before["FragmentPath"]).resolve(strict=True)
            ),
        },
        "isolation": {
            "provider": "linux_hardened_v1",
            "threatBoundary": "hostile workload on a trusted host kernel",
            "cgroupRoot": str(cgroup_root),
            "exclusiveChildrenBeforeAndAfter": ["service"],
            "delegatedControllers": sorted(REQUIRED_CONTROLLERS),
            "delegateSubgroup": "service",
            "runtime": str(runtime),
        },
        "workloadSizing": {
            "scenarios": {
                "memoryBytes": 256 * 1024 * 1024,
                "childProcesses": 16,
                "cpuBusyMilliseconds": 750,
            },
            "observed": metrics,
            "configuredHeadroom": {
                "memoryRatioAtLeast": 4.0,
                "pidsRatioAtLeast": 64 / 17,
                "cpuQuotaCores": 1.0,
            },
            "testBinary": test_identity,
            "test": TEST_NAME,
        },
        "notClaimed": [
            "Containment of a hostile or compromised host kernel.",
            "MicroVM isolation; the selected owner-controlled target trusts its host kernel.",
            "P6.6 physical Android speech-quality acceptance.",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--test-binary", required=True)
    parser.add_argument("--expected-hostname", default="homolab")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = Path(args.output)
    if output.exists() or output.is_symlink():
        fail("refusing to overwrite an existing production report")
    report = accept(args)
    temporary = output.with_name(f".{output.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.chmod(temporary, 0o644)
    os.replace(temporary, output)
    print(f"M4 production acceptance report: {output}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AcceptanceError as error:
        print(f"m4-production-acceptance: {error}", file=sys.stderr)
        raise SystemExit(1)
