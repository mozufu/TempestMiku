#!/usr/bin/env python3
"""Fail-closed M4 disposable native-architecture acceptance kit.

The v1 report deliberately cannot represent production-service acceptance.  It
binds a root-owned prebuilt test executable, an exact clean Git revision, the
operator-authored contract and host config, trusted runtime contents, and a
bounded sanitized execution.  Production service semantics, cgroup ownership
exclusivity, workload sizing, hostile-host-kernel containment, and microVM
assurance remain separate open gates.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Callable, NoReturn


CONTRACT_SCHEMA_VERSION = 1
REPORT_SCHEMA_VERSION = 1
REPORT_TYPE = "tempestmiku.m4.linux_hardened_v1.acceptance"
EVIDENCE_CLASS = "disposable_native_architecture"
SUPPORTED_ARCHITECTURES = {"aarch64", "x86_64"}
SERVICE_ROLES = {"api", "worker", "all"}
REQUIRED_CONTROLLERS = {"cpu", "memory", "pids"}
MAX_CANARY_OUTPUT_BYTES = 1024 * 1024
MAX_CANARY_TIMEOUT_SECONDS = 600
CANARY_TEST_NAME = (
    "linked::tests::linux_isolation::"
    "gated_linux_hardened_v1_enforces_seccomp_and_cgroup_lifecycle"
)
CANARY_ARGUMENTS = [
    CANARY_TEST_NAME,
    "--exact",
    "--test-threads=1",
    "--nocapture",
]
EXECUTION_ENV_KEYS = {"PATH", "HOME", "TMPDIR"}
TOOL_NAMES = {
    "python",
    "git",
    "bash",
    "wrapper",
    "acceptanceProgram",
    "canaryExecutable",
}

CONTRACT_TOP_KEYS = {
    "schemaVersion",
    "evidenceClass",
    "threatModel",
    "microvm",
    "service",
    "isolation",
    "tools",
    "execution",
    "source",
    "workloadSizing",
}
THREAT_MODEL_KEYS = {"hostKernel", "workload", "hostileHostKernelClaimed"}
MICROVM_KEYS = {"required", "selected", "provider", "evidencePath"}
SERVICE_KEYS = {
    "name",
    "role",
    "expectedUid",
    "expectedGid",
    "expectedArchitecture",
    "hostConfigPath",
    "serviceManagerConfigPath",
    "serverBinaryPath",
    "runtimeArtifactIdentity",
    "expectedEnvironment",
}
ISOLATION_KEYS = {
    "provider",
    "cgroupRoot",
    "launcher",
    "runtimeRoots",
    "linkedRoots",
    "exclusivity",
}
EXCLUSIVITY_KEYS = {"status", "reason"}
TOOL_SPEC_KEYS = {"path", "sha256"}
EXECUTION_KEYS = {"timeoutSeconds", "environment"}
SOURCE_KEYS = {"repositoryRoot", "expectedRevision", "requireClean"}
WORKLOAD_SIZING_KEYS = {"status", "reason"}

REPORT_TOP_KEYS = {
    "schemaVersion",
    "reportType",
    "status",
    "recordedAt",
    "evidenceClass",
    "threatModel",
    "microvm",
    "workloadSizing",
    "contract",
    "hostConfig",
    "service",
    "tools",
    "isolation",
    "canary",
    "git",
    "sourceSha256",
    "assertions",
    "scopeBoundary",
    "bindingSha256",
}
REPORT_ASSERTION_KEYS = {
    "contractValidated",
    "operatorIdentityMatched",
    "trustedFilesConstrained",
    "hostConfigMatchedContract",
    "hostKernelThreatModelTrusted",
    "cleanExpectedRevision",
    "toolIdentitiesStable",
    "canaryEnvironmentSanitized",
    "serviceOutsideDelegatedRoot",
    "delegatedRootObservedEmptyBefore",
    "delegatedRootObservedEmptyAfter",
    "cgroupExclusivityNotClaimed",
    "controllersDelegated",
    "rlimitsReadBack",
    "cgroupLimitsReadBack",
    "runtimeContentsStable",
    "sourceStableDuringCanary",
    "boundedExactCanaryPassed",
    "outputNotRetained",
    "reportValidated",
}

SOURCE_PATHS = (
    ".github/workflows/m4-native-x86-canary.yml",
    "Cargo.lock",
    "Cargo.toml",
    "crates/tm-host/Cargo.toml",
    "crates/tm-host/src/linked/config.rs",
    "crates/tm-host/src/linked/isolation.rs",
    "crates/tm-host/src/linked/isolation/cgroup.rs",
    "crates/tm-host/src/linked/isolation/seccomp.rs",
    "crates/tm-host/src/linked/tools/proc.rs",
    "crates/tm-host/src/linked/tools/proc/bounded_io.rs",
    "crates/tm-host/src/linked/tools/proc/environment.rs",
    "crates/tm-host/src/linked/tools/proc/process_group.rs",
    "crates/tm-host/src/linked/tests/linux_isolation.rs",
    "tools/m4-linux-hardened-canary.sh",
    "tools/m4-native-x86-disposable-canary.sh",
    "tools/m4-resource-probe.c",
    "tools/m4-thread-probe.c",
    "tools/m4_acceptance.py",
    "tools/m4-deployment-contract.schema.json",
    "tools/m4-acceptance-report.schema.json",
    "tools/m4-deployment-contract.example.json",
    "tools/tests/test_m4_acceptance.py",
)


class ValidationError(ValueError):
    """A deterministic contract, report, or live-gate failure."""


def fail(message: str) -> NoReturn:
    raise ValidationError(message)


def _require_object(
    value: Any, label: str, exact_keys: set[str] | None = None
) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    if exact_keys is not None:
        actual = set(value)
        missing = sorted(exact_keys - actual)
        unknown = sorted(actual - exact_keys)
        if missing:
            fail(f"{label} is missing required fields: {', '.join(missing)}")
        if unknown:
            fail(f"{label} contains unknown fields: {', '.join(unknown)}")
    return value


def _require_array(value: Any, label: str, *, nonempty: bool = False) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{label} must be an array")
    if nonempty and not value:
        fail(f"{label} must not be empty")
    return value


def _require_string(value: Any, label: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str):
        fail(f"{label} must be a string")
    if nonempty and not value.strip():
        fail(f"{label} must not be empty")
    if "\x00" in value or "\n" in value or "\r" in value:
        fail(f"{label} must be a single NUL-free line")
    return value


def _require_bool(value: Any, label: str) -> bool:
    if not isinstance(value, bool):
        fail(f"{label} must be a boolean")
    return value


def _require_int(
    value: Any, label: str, *, minimum: int | None = None, maximum: int | None = None
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        fail(f"{label} must be an integer")
    if minimum is not None and value < minimum:
        fail(f"{label} must be at least {minimum}")
    if maximum is not None and value > maximum:
        fail(f"{label} must be at most {maximum}")
    return value


def _require_nullable_string(value: Any, label: str) -> str | None:
    if value is None:
        return None
    return _require_string(value, label)


def _require_absolute_path(value: Any, label: str) -> str:
    text = _require_string(value, label)
    if not Path(text).is_absolute():
        fail(f"{label} must be an absolute path")
    if os.path.normpath(text) != text:
        fail(f"{label} must be normalized without '.' or '..' components")
    return text


def _require_unique_absolute_paths(value: Any, label: str) -> list[str]:
    items = _require_array(value, label, nonempty=True)
    paths = [
        _require_absolute_path(item, f"{label}[{index}]")
        for index, item in enumerate(items)
    ]
    if len(paths) != len(set(paths)):
        fail(f"{label} must not contain duplicate paths")
    return paths


def _validate_sha256(value: Any, label: str) -> str:
    text = _require_string(value, label)
    if re.fullmatch(r"[0-9a-f]{64}", text) is None:
        fail(f"{label} must be a lowercase SHA-256 hex digest")
    return text


def _require_runtime_artifact_digest(value: Any, label: str) -> str:
    text = _require_string(value, label)
    if re.search(r"(?:^|@)sha256:[0-9a-f]{64}$", text) is None:
        fail(f"{label} must end in a lowercase sha256:<64-hex> digest")
    return text


def _runtime_artifact_sha256(value: str) -> str:
    return value.rsplit("sha256:", 1)[1]


def _canonical_digest(value: Any) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _identity_digest(identity: dict[str, Any]) -> str:
    return _canonical_digest(identity)


def _reject_duplicate_json_fields(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            fail(f"JSON object contains duplicate field {key!r}")
        value[key] = item
    return value


def _reject_nonfinite_json_number(value: str) -> NoReturn:
    fail(f"JSON contains non-finite number {value}")


def load_json(path: Path) -> Any:
    try:
        raw = path.read_bytes()
    except OSError as error:
        fail(f"cannot read {path}: {error}")
    try:
        return json.loads(
            raw,
            object_pairs_hook=_reject_duplicate_json_fields,
            parse_constant=_reject_nonfinite_json_number,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{path} is not valid UTF-8 JSON: {error}")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path}: {error}")
    return digest.hexdigest()


def _validate_tool_spec(value: Any, label: str) -> dict[str, Any]:
    spec = _require_object(value, label, TOOL_SPEC_KEYS)
    _require_absolute_path(spec["path"], f"{label}.path")
    _validate_sha256(spec["sha256"], f"{label}.sha256")
    return spec


def _validate_execution_environment(value: Any, label: str) -> dict[str, str]:
    environment = _require_object(value, label, EXECUTION_ENV_KEYS)
    for key in ("HOME", "TMPDIR"):
        _require_absolute_path(environment[key], f"{label}.{key}")
    path_value = _require_string(environment["PATH"], f"{label}.PATH")
    entries = path_value.split(os.pathsep)
    if not entries or any(not entry for entry in entries):
        fail(f"{label}.PATH must contain only non-empty absolute entries")
    for index, entry in enumerate(entries):
        _require_absolute_path(entry, f"{label}.PATH[{index}]")
    return environment


def validate_contract(value: Any, *, live: bool = False) -> dict[str, Any]:
    contract = _require_object(value, "contract", CONTRACT_TOP_KEYS)
    if _require_int(contract["schemaVersion"], "contract.schemaVersion") != 1:
        fail("contract.schemaVersion must be 1")
    if _require_string(contract["evidenceClass"], "contract.evidenceClass") != EVIDENCE_CLASS:
        fail(
            "acceptance-kit v1 refuses production_service evidence; use "
            "disposable_native_architecture and keep production acceptance open"
        )

    threat = _require_object(
        contract["threatModel"], "contract.threatModel", THREAT_MODEL_KEYS
    )
    if _require_string(threat["hostKernel"], "contract.threatModel.hostKernel") != "trusted":
        fail("linux_hardened_v1 requires a trusted host kernel; hostile kernels require a microVM")
    if _require_string(threat["workload"], "contract.threatModel.workload") != "hostile":
        fail("contract.threatModel.workload must be hostile")
    if _require_bool(
        threat["hostileHostKernelClaimed"],
        "contract.threatModel.hostileHostKernelClaimed",
    ):
        fail("linux_hardened_v1 must not claim hostile host-kernel containment")

    microvm = _require_object(contract["microvm"], "contract.microvm", MICROVM_KEYS)
    required = _require_bool(microvm["required"], "contract.microvm.required")
    selected = _require_bool(microvm["selected"], "contract.microvm.selected")
    provider = _require_nullable_string(microvm["provider"], "contract.microvm.provider")
    evidence_path = _require_nullable_string(
        microvm["evidencePath"], "contract.microvm.evidencePath"
    )
    if required and not selected:
        fail("contract.microvm.selected must be true when microvm.required is true")
    if selected and (provider is None or evidence_path is None):
        fail("a selected microVM requires provider and evidencePath")
    if not selected and (provider is not None or evidence_path is not None):
        fail("unselected microVM fields provider and evidencePath must be null")
    if evidence_path is not None:
        _require_absolute_path(evidence_path, "contract.microvm.evidencePath")

    service = _require_object(contract["service"], "contract.service", SERVICE_KEYS)
    _require_string(service["name"], "contract.service.name")
    role = _require_string(service["role"], "contract.service.role")
    if role not in SERVICE_ROLES:
        fail("contract.service.role must be api, worker, or all")
    _require_int(service["expectedUid"], "contract.service.expectedUid", minimum=1)
    _require_int(service["expectedGid"], "contract.service.expectedGid", minimum=1)
    architecture = _require_string(
        service["expectedArchitecture"], "contract.service.expectedArchitecture"
    )
    if architecture not in SUPPORTED_ARCHITECTURES:
        fail("contract.service.expectedArchitecture must be aarch64 or x86_64")
    for field in ("hostConfigPath", "serviceManagerConfigPath", "serverBinaryPath"):
        _require_absolute_path(service[field], f"contract.service.{field}")
    artifact = _require_runtime_artifact_digest(
        service["runtimeArtifactIdentity"], "contract.service.runtimeArtifactIdentity"
    )
    if live and ("replace" in artifact.lower() or "example" in artifact.lower()):
        fail("contract.service.runtimeArtifactIdentity still contains a placeholder")
    expected_environment = _require_object(
        service["expectedEnvironment"],
        "contract.service.expectedEnvironment",
        {"TM_HOST_CONFIG", "TM_SERVER_ROLE"},
    )
    if _require_absolute_path(
        expected_environment["TM_HOST_CONFIG"],
        "contract.service.expectedEnvironment.TM_HOST_CONFIG",
    ) != service["hostConfigPath"]:
        fail("contract service TM_HOST_CONFIG must equal hostConfigPath")
    if _require_string(
        expected_environment["TM_SERVER_ROLE"],
        "contract.service.expectedEnvironment.TM_SERVER_ROLE",
    ) != role:
        fail("contract service TM_SERVER_ROLE must equal role")

    isolation = _require_object(
        contract["isolation"], "contract.isolation", ISOLATION_KEYS
    )
    if _require_string(isolation["provider"], "contract.isolation.provider") != "linux_hardened_v1":
        fail("contract.isolation.provider must be linux_hardened_v1")
    cgroup_root = _require_absolute_path(
        isolation["cgroupRoot"], "contract.isolation.cgroupRoot"
    )
    if cgroup_root == "/sys/fs/cgroup" or not cgroup_root.startswith("/sys/fs/cgroup/"):
        fail("contract.isolation.cgroupRoot must be a dedicated subtree below /sys/fs/cgroup")
    _require_absolute_path(isolation["launcher"], "contract.isolation.launcher")
    _require_unique_absolute_paths(
        isolation["runtimeRoots"], "contract.isolation.runtimeRoots"
    )
    linked = _require_array(
        isolation["linkedRoots"], "contract.isolation.linkedRoots", nonempty=True
    )
    linked_paths: list[str] = []
    for index, item in enumerate(linked):
        root = _require_object(
            item,
            f"contract.isolation.linkedRoots[{index}]",
            {"path", "mode"},
        )
        linked_paths.append(
            _require_absolute_path(
                root["path"], f"contract.isolation.linkedRoots[{index}].path"
            )
        )
        if _require_string(
            root["mode"], f"contract.isolation.linkedRoots[{index}].mode"
        ) not in {"ro", "rw"}:
            fail(f"contract.isolation.linkedRoots[{index}].mode must be ro or rw")
    if len(linked_paths) != len(set(linked_paths)):
        fail("contract.isolation.linkedRoots must not contain duplicate paths")
    exclusivity = _require_object(
        isolation["exclusivity"],
        "contract.isolation.exclusivity",
        EXCLUSIVITY_KEYS,
    )
    if _require_string(
        exclusivity["status"], "contract.isolation.exclusivity.status"
    ) != "not_proven":
        fail("acceptance-kit v1 requires cgroup exclusivity status=not_proven")
    _require_string(exclusivity["reason"], "contract.isolation.exclusivity.reason")

    tools = _require_object(contract["tools"], "contract.tools", TOOL_NAMES)
    tool_paths: list[str] = []
    for name in sorted(TOOL_NAMES):
        spec = _validate_tool_spec(tools[name], f"contract.tools.{name}")
        tool_paths.append(spec["path"])
    if len(tool_paths) != len(set(tool_paths)):
        fail("contract.tools paths must be unique")

    execution = _require_object(
        contract["execution"], "contract.execution", EXECUTION_KEYS
    )
    _require_int(
        execution["timeoutSeconds"],
        "contract.execution.timeoutSeconds",
        minimum=1,
        maximum=MAX_CANARY_TIMEOUT_SECONDS,
    )
    _validate_execution_environment(
        execution["environment"], "contract.execution.environment"
    )

    source = _require_object(contract["source"], "contract.source", SOURCE_KEYS)
    _require_absolute_path(source["repositoryRoot"], "contract.source.repositoryRoot")
    revision = _require_string(source["expectedRevision"], "contract.source.expectedRevision")
    if re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        fail("contract.source.expectedRevision must be a full lowercase Git revision")
    if not _require_bool(source["requireClean"], "contract.source.requireClean"):
        fail("acceptance-kit v1 requires contract.source.requireClean=true")

    sizing = _require_object(
        contract["workloadSizing"],
        "contract.workloadSizing",
        WORKLOAD_SIZING_KEYS,
    )
    if _require_string(sizing["status"], "contract.workloadSizing.status") != "pending":
        fail("acceptance-kit v1 requires workloadSizing.status=pending")
    _require_string(sizing["reason"], "contract.workloadSizing.reason")
    return contract


def _validate_identity(
    value: Any, label: str, *, kind: str = "directory"
) -> dict[str, Any]:
    identity = _require_object(value, label)
    required = {
        "configuredPath",
        "canonicalPath",
        "device",
        "inode",
        "uid",
        "gid",
        "mode",
        "identitySha256",
    }
    if kind == "file":
        required |= {"contentSha256", "sizeBytes"}
    elif kind == "tree":
        required |= {"treeSha256", "entryCount", "totalBytes"}
    elif kind != "directory":
        fail(f"unsupported identity kind {kind}")
    actual = set(identity)
    if actual != required:
        missing = sorted(required - actual)
        unknown = sorted(actual - required)
        if missing:
            fail(f"{label} is missing required fields: {', '.join(missing)}")
        fail(f"{label} contains unknown fields: {', '.join(unknown)}")
    _require_absolute_path(identity["configuredPath"], f"{label}.configuredPath")
    _require_absolute_path(identity["canonicalPath"], f"{label}.canonicalPath")
    for field in ("device", "inode", "uid", "gid", "mode"):
        _require_int(identity[field], f"{label}.{field}", minimum=0)
    base = {
        field: identity[field]
        for field in (
            "configuredPath",
            "canonicalPath",
            "device",
            "inode",
            "uid",
            "gid",
            "mode",
        )
    }
    if _identity_digest(base) != _validate_sha256(
        identity["identitySha256"], f"{label}.identitySha256"
    ):
        fail(f"{label}.identitySha256 does not match the identity fields")
    if kind == "file":
        _validate_sha256(identity["contentSha256"], f"{label}.contentSha256")
        _require_int(identity["sizeBytes"], f"{label}.sizeBytes", minimum=0)
    if kind == "tree":
        _validate_sha256(identity["treeSha256"], f"{label}.treeSha256")
        _require_int(identity["entryCount"], f"{label}.entryCount", minimum=1)
        _require_int(identity["totalBytes"], f"{label}.totalBytes", minimum=0)
    return identity


def _require_trusted_report_identity(
    identity: dict[str, Any], label: str, *, executable: bool = False
) -> None:
    if identity["uid"] != 0 or identity["mode"] & 0o022:
        fail(f"{label} must be root-owned and not group/world writable")
    if executable and identity["mode"] & 0o111 == 0:
        fail(f"{label} must record an executable identity")


def _validate_microvm_report(value: Any) -> dict[str, Any]:
    microvm = _require_object(value, "report.microvm", MICROVM_KEYS)
    required = _require_bool(microvm["required"], "report.microvm.required")
    selected = _require_bool(microvm["selected"], "report.microvm.selected")
    provider = _require_nullable_string(microvm["provider"], "report.microvm.provider")
    evidence = _require_nullable_string(
        microvm["evidencePath"], "report.microvm.evidencePath"
    )
    if required and not selected:
        fail("report.microvm.selected must be true when required")
    if selected and (provider is None or evidence is None):
        fail("report selected microVM must include provider and evidencePath")
    if not selected and (provider is not None or evidence is not None):
        fail("report unselected microVM fields provider and evidencePath must be null")
    if evidence is not None:
        _require_absolute_path(evidence, "report.microvm.evidencePath")
    return microvm


def _report_binding_digest(report: dict[str, Any]) -> str:
    unbound = {key: value for key, value in report.items() if key != "bindingSha256"}
    return _canonical_digest(unbound)


def validate_report(value: Any) -> dict[str, Any]:
    report = _require_object(value, "report", REPORT_TOP_KEYS)
    if _require_int(report["schemaVersion"], "report.schemaVersion") != 1:
        fail("report.schemaVersion must be 1")
    if _require_string(report["reportType"], "report.reportType") != REPORT_TYPE:
        fail(f"report.reportType must be {REPORT_TYPE}")
    if _require_string(report["status"], "report.status") != "passed":
        fail("an M4 acceptance report is valid only when status is passed")
    recorded_at = _require_string(report["recordedAt"], "report.recordedAt")
    if re.fullmatch(
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})",
        recorded_at,
    ) is None:
        fail("report.recordedAt must include an explicit timezone")
    try:
        dt.datetime.fromisoformat(recorded_at.replace("Z", "+00:00"))
    except ValueError:
        fail("report.recordedAt must be an ISO-8601 timestamp")
    if _require_string(report["evidenceClass"], "report.evidenceClass") != EVIDENCE_CLASS:
        fail("acceptance-kit v1 reports cannot claim production_service evidence")

    threat = _require_object(
        report["threatModel"], "report.threatModel", THREAT_MODEL_KEYS
    )
    if _require_string(threat["hostKernel"], "report.threatModel.hostKernel") != "trusted":
        fail("report.threatModel.hostKernel must be trusted")
    if _require_string(threat["workload"], "report.threatModel.workload") != "hostile":
        fail("report.threatModel.workload must be hostile")
    if _require_bool(
        threat["hostileHostKernelClaimed"],
        "report.threatModel.hostileHostKernelClaimed",
    ):
        fail("report must not claim hostile host-kernel containment")
    _validate_microvm_report(report["microvm"])

    sizing = _require_object(
        report["workloadSizing"],
        "report.workloadSizing",
        WORKLOAD_SIZING_KEYS,
    )
    if _require_string(sizing["status"], "report.workloadSizing.status") != "pending":
        fail("report.workloadSizing.status must remain pending")
    _require_string(sizing["reason"], "report.workloadSizing.reason")

    contract = _require_object(
        report["contract"],
        "report.contract",
        {"identity", "schemaVersion", "canonicalSha256"},
    )
    contract_identity = _validate_identity(
        contract["identity"], "report.contract.identity", kind="file"
    )
    _require_trusted_report_identity(contract_identity, "report.contract.identity")
    if _require_int(contract["schemaVersion"], "report.contract.schemaVersion") != 1:
        fail("report.contract.schemaVersion must be 1")
    _validate_sha256(contract["canonicalSha256"], "report.contract.canonicalSha256")

    host_config = _require_object(
        report["hostConfig"],
        "report.hostConfig",
        {"identity", "provider", "rlimits", "cgroupLimits"},
    )
    host_identity = _validate_identity(
        host_config["identity"], "report.hostConfig.identity", kind="file"
    )
    _require_trusted_report_identity(host_identity, "report.hostConfig.identity")
    if _require_string(host_config["provider"], "report.hostConfig.provider") != "linux_hardened_v1":
        fail("report.hostConfig.provider must be linux_hardened_v1")
    for field_name, expected_fields, zero_allowed in (
        (
            "rlimits",
            {"addressSpaceBytes", "processCount", "openFiles"},
            set(),
        ),
        (
            "cgroupLimits",
            {
                "memoryMaxBytes",
                "memorySwapMaxBytes",
                "pidsMax",
                "cpuQuotaMicros",
                "cpuPeriodMicros",
            },
            {"memorySwapMaxBytes"},
        ),
    ):
        limits = _require_object(
            host_config[field_name], f"report.hostConfig.{field_name}", expected_fields
        )
        for field in expected_fields:
            minimum = 0 if field in zero_allowed else 1
            _require_int(
                limits[field], f"report.hostConfig.{field_name}.{field}", minimum=minimum
            )

    service = _require_object(
        report["service"],
        "report.service",
        {
            "name",
            "role",
            "uid",
            "gid",
            "architecture",
            "kernel",
            "runtimeArtifactIdentity",
            "environment",
            "semantics",
            "serverBinary",
            "serviceManagerConfig",
            "currentCgroupPath",
            "cgroupNamespace",
        },
    )
    _require_string(service["name"], "report.service.name")
    if _require_string(service["role"], "report.service.role") not in SERVICE_ROLES:
        fail("report.service.role is unsupported")
    _require_int(service["uid"], "report.service.uid", minimum=1)
    _require_int(service["gid"], "report.service.gid", minimum=1)
    if _require_string(service["architecture"], "report.service.architecture") not in SUPPORTED_ARCHITECTURES:
        fail("report.service.architecture is unsupported")
    _require_string(service["kernel"], "report.service.kernel")
    artifact = _require_runtime_artifact_digest(
        service["runtimeArtifactIdentity"], "report.service.runtimeArtifactIdentity"
    )
    environment = _require_object(
        service["environment"],
        "report.service.environment",
        {"TM_HOST_CONFIG", "TM_SERVER_ROLE"},
    )
    _require_absolute_path(
        environment["TM_HOST_CONFIG"], "report.service.environment.TM_HOST_CONFIG"
    )
    _require_string(
        environment["TM_SERVER_ROLE"], "report.service.environment.TM_SERVER_ROLE"
    )
    if _require_string(service["semantics"], "report.service.semantics") != "disposable_fixture_only":
        fail("report.service.semantics must remain disposable_fixture_only")
    server = _validate_identity(
        service["serverBinary"], "report.service.serverBinary", kind="file"
    )
    _require_trusted_report_identity(
        server, "report.service.serverBinary", executable=True
    )
    if server["contentSha256"] != _runtime_artifact_sha256(artifact):
        fail("report runtime artifact must bind the exact server-binary content")
    manager = _validate_identity(
        service["serviceManagerConfig"],
        "report.service.serviceManagerConfig",
        kind="file",
    )
    _require_trusted_report_identity(manager, "report.service.serviceManagerConfig")
    _require_absolute_path(
        service["currentCgroupPath"], "report.service.currentCgroupPath"
    )
    namespace = _require_object(
        service["cgroupNamespace"],
        "report.service.cgroupNamespace",
        {"device", "inode"},
    )
    for field in ("device", "inode"):
        _require_int(namespace[field], f"report.service.cgroupNamespace.{field}", minimum=0)

    tools = _require_object(report["tools"], "report.tools", TOOL_NAMES)
    for name in sorted(TOOL_NAMES):
        tool = _validate_identity(tools[name], f"report.tools.{name}", kind="file")
        _require_trusted_report_identity(
            tool,
            f"report.tools.{name}",
            executable=name != "acceptanceProgram",
        )

    isolation = _require_object(
        report["isolation"],
        "report.isolation",
        {"provider", "launcher", "runtimeRoots", "linkedRoots", "cgroup"},
    )
    if _require_string(isolation["provider"], "report.isolation.provider") != "linux_hardened_v1":
        fail("report.isolation.provider must be linux_hardened_v1")
    launcher = _validate_identity(
        isolation["launcher"], "report.isolation.launcher", kind="file"
    )
    _require_trusted_report_identity(
        launcher, "report.isolation.launcher", executable=True
    )
    runtime_roots = _require_array(
        isolation["runtimeRoots"], "report.isolation.runtimeRoots", nonempty=True
    )
    runtime_paths: list[str] = []
    for index, item in enumerate(runtime_roots):
        identity = _validate_identity(
            item, f"report.isolation.runtimeRoots[{index}]", kind="tree"
        )
        _require_trusted_report_identity(
            identity, f"report.isolation.runtimeRoots[{index}]"
        )
        runtime_paths.append(identity["configuredPath"])
    if len(runtime_paths) != len(set(runtime_paths)):
        fail("report.isolation.runtimeRoots contains duplicate paths")
    linked_roots = _require_array(
        isolation["linkedRoots"], "report.isolation.linkedRoots", nonempty=True
    )
    linked_paths: list[str] = []
    for index, item in enumerate(linked_roots):
        entry = _require_object(
            item,
            f"report.isolation.linkedRoots[{index}]",
            {"mode", "identity"},
        )
        if _require_string(
            entry["mode"], f"report.isolation.linkedRoots[{index}].mode"
        ) not in {"ro", "rw"}:
            fail(f"report.isolation.linkedRoots[{index}].mode must be ro or rw")
        identity = _validate_identity(
            entry["identity"],
            f"report.isolation.linkedRoots[{index}].identity",
        )
        linked_paths.append(identity["configuredPath"])
    if len(linked_paths) != len(set(linked_paths)):
        fail("report.isolation.linkedRoots contains duplicate paths")

    cgroup = _require_object(
        isolation["cgroup"],
        "report.isolation.cgroup",
        {
            "path",
            "device",
            "inode",
            "uid",
            "gid",
            "mountPoint",
            "controllers",
            "subtreeControl",
            "killWritable",
            "emptyBefore",
            "emptyAfter",
            "unknownChildrenBefore",
            "unknownChildrenAfter",
            "exclusivity",
        },
    )
    for field in ("path", "mountPoint"):
        _require_absolute_path(cgroup[field], f"report.isolation.cgroup.{field}")
    if cgroup["path"] == "/sys/fs/cgroup" or not cgroup["path"].startswith("/sys/fs/cgroup/"):
        fail("report cgroup path must be a dedicated subtree")
    for field in ("device", "inode", "uid", "gid"):
        _require_int(cgroup[field], f"report.isolation.cgroup.{field}", minimum=0)
    for field in ("controllers", "subtreeControl"):
        values = _require_array(
            cgroup[field], f"report.isolation.cgroup.{field}", nonempty=True
        )
        for index, item in enumerate(values):
            _require_string(item, f"report.isolation.cgroup.{field}[{index}]")
        if values != sorted(set(values)):
            fail(f"report.isolation.cgroup.{field} must be sorted and unique")
        if not REQUIRED_CONTROLLERS.issubset(set(values)):
            fail(f"report.isolation.cgroup.{field} is missing a required controller")
    for field in ("killWritable", "emptyBefore", "emptyAfter"):
        if not _require_bool(cgroup[field], f"report.isolation.cgroup.{field}"):
            fail(f"report.isolation.cgroup.{field} must be true")
    for field in ("unknownChildrenBefore", "unknownChildrenAfter"):
        if _require_array(cgroup[field], f"report.isolation.cgroup.{field}"):
            fail(f"report.isolation.cgroup.{field} must be empty")
    exclusivity = _require_object(
        cgroup["exclusivity"],
        "report.isolation.cgroup.exclusivity",
        EXCLUSIVITY_KEYS,
    )
    if _require_string(
        exclusivity["status"], "report.isolation.cgroup.exclusivity.status"
    ) != "not_proven":
        fail("report must not claim cgroup exclusivity without a supervisor lease")
    _require_string(exclusivity["reason"], "report.isolation.cgroup.exclusivity.reason")
    if cgroup["uid"] != service["uid"] or cgroup["gid"] != service["gid"]:
        fail("report cgroup ownership must match the canary identity")
    try:
        Path(service["currentCgroupPath"]).relative_to(Path(cgroup["path"]))
    except ValueError:
        pass
    else:
        fail("report canary process must remain outside the delegated root")

    canary = _require_object(
        report["canary"],
        "report.canary",
        {
            "arguments",
            "status",
            "durationMs",
            "timeoutSeconds",
            "outputBytes",
            "outputSha256",
            "outputRetained",
            "successSummaryMatched",
            "environmentKeys",
            "environmentSha256",
        },
    )
    if canary["arguments"] != CANARY_ARGUMENTS:
        fail("report.canary.arguments must select the exact hardened test")
    if _require_string(canary["status"], "report.canary.status") != "passed":
        fail("report.canary.status must be passed")
    _require_int(canary["durationMs"], "report.canary.durationMs", minimum=0)
    _require_int(
        canary["timeoutSeconds"],
        "report.canary.timeoutSeconds",
        minimum=1,
        maximum=MAX_CANARY_TIMEOUT_SECONDS,
    )
    _require_int(
        canary["outputBytes"],
        "report.canary.outputBytes",
        minimum=1,
        maximum=MAX_CANARY_OUTPUT_BYTES,
    )
    _validate_sha256(canary["outputSha256"], "report.canary.outputSha256")
    if _require_bool(canary["outputRetained"], "report.canary.outputRetained"):
        fail("report must not retain raw canary output")
    if not _require_bool(
        canary["successSummaryMatched"], "report.canary.successSummaryMatched"
    ):
        fail("report.canary.successSummaryMatched must be true")
    environment_keys = _require_array(
        canary["environmentKeys"], "report.canary.environmentKeys", nonempty=True
    )
    if environment_keys != sorted(EXECUTION_ENV_KEYS | {"TM_HOST_CONFIG", "TM_LINUX_HARDENED_TESTS"}):
        fail("report.canary.environmentKeys must equal the sanitized allowlist")
    _validate_sha256(canary["environmentSha256"], "report.canary.environmentSha256")

    git = _require_object(
        report["git"], "report.git", {"revision", "dirty", "statusShort"}
    )
    revision = _require_string(git["revision"], "report.git.revision")
    if re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        fail("report.git.revision must be a full lowercase Git revision")
    if _require_bool(git["dirty"], "report.git.dirty"):
        fail("acceptance reports require a clean source revision")
    if _require_array(git["statusShort"], "report.git.statusShort"):
        fail("report.git.statusShort must be empty for a clean revision")

    hashes = _require_object(report["sourceSha256"], "report.sourceSha256")
    if set(hashes) != set(SOURCE_PATHS):
        missing = sorted(set(SOURCE_PATHS) - set(hashes))
        unknown = sorted(set(hashes) - set(SOURCE_PATHS))
        if missing:
            fail("report.sourceSha256 is missing required paths: " + ", ".join(missing))
        fail("report.sourceSha256 contains unknown paths: " + ", ".join(unknown))
    for path, digest in hashes.items():
        _validate_sha256(digest, f"report.sourceSha256[{path}]")

    assertions = _require_object(
        report["assertions"], "report.assertions", REPORT_ASSERTION_KEYS
    )
    for field in REPORT_ASSERTION_KEYS:
        if _require_string(assertions[field], f"report.assertions.{field}") != "passed":
            fail(f"report.assertions.{field} must be passed")

    scope = _require_object(
        report["scopeBoundary"],
        "report.scopeBoundary",
        {"proven", "notProven"},
    )
    proven = _require_array(scope["proven"], "report.scopeBoundary.proven", nonempty=True)
    not_proven = _require_array(
        scope["notProven"], "report.scopeBoundary.notProven", nonempty=True
    )
    for label, items in (("proven", proven), ("notProven", not_proven)):
        for index, item in enumerate(items):
            _require_string(item, f"report.scopeBoundary.{label}[{index}]")
    lower_proven = " ".join(proven).lower()
    for forbidden in (
        "production service",
        "production-service",
        "cgroup exclusivity",
        "exclusive delegated",
        "hostile host-kernel",
        "microvm isolation",
        "workload sizing",
    ):
        if forbidden in lower_proven:
            fail(f"report.scopeBoundary.proven must not claim {forbidden}")
    lower_not_proven = " ".join(not_proven).lower()
    for required_text in (
        "production service",
        "cgroup exclusivity",
        "workload sizing",
        "hostile host-kernel",
        "microvm isolation",
    ):
        if required_text not in lower_not_proven:
            fail(f"report.scopeBoundary.notProven must retain {required_text}")

    binding = _validate_sha256(report["bindingSha256"], "report.bindingSha256")
    if binding != _report_binding_digest(report):
        fail("report.bindingSha256 does not match the complete report")
    return report


def _trusted_ancestry(path: Path, label: str) -> None:
    current = path.parent
    while True:
        try:
            metadata = current.stat()
        except OSError as error:
            fail(f"{label} ancestor {current} cannot be inspected: {error}")
        mode = stat.S_IMODE(metadata.st_mode)
        sticky_root = metadata.st_uid == 0 and bool(mode & stat.S_ISVTX)
        if (
            not stat.S_ISDIR(metadata.st_mode)
            or metadata.st_uid != 0
            or (mode & 0o022 and not sticky_root)
        ):
            fail(
                f"{label} ancestor {current} must be root-owned and not group/world "
                "writable (except a root-owned sticky directory)"
            )
        if current == current.parent:
            break
        current = current.parent


def secure_file_identity(
    path_text: str, label: str, *, executable: bool = False
) -> dict[str, Any]:
    path = Path(_require_absolute_path(path_text, label))
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"{label} {path} cannot be inspected: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} {path} must be a regular non-symlink file")
    mode = stat.S_IMODE(metadata.st_mode)
    if metadata.st_uid != 0 or mode & 0o022:
        fail(f"{label} {path} must be root-owned and not group/world writable")
    if executable and mode & 0o111 == 0:
        fail(f"{label} {path} must be executable")
    _trusted_ancestry(path, label)
    canonical = path.resolve(strict=True)
    base = {
        "configuredPath": str(path),
        "canonicalPath": str(canonical),
        "device": metadata.st_dev,
        "inode": metadata.st_ino,
        "uid": metadata.st_uid,
        "gid": metadata.st_gid,
        "mode": mode,
    }
    return {
        **base,
        "identitySha256": _identity_digest(base),
        "contentSha256": sha256_file(path),
        "sizeBytes": metadata.st_size,
    }


def _manifest_record(
    digest: "hashlib._Hash", relative: str, fields: list[str]
) -> None:
    encoded = json.dumps(
        [relative, *fields], separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    digest.update(len(encoded).to_bytes(8, "big"))
    digest.update(encoded)


def tree_identity(path_text: str, label: str) -> dict[str, Any]:
    configured = Path(_require_absolute_path(path_text, label))
    try:
        root_metadata = configured.lstat()
    except OSError as error:
        fail(f"{label} {configured} cannot be inspected: {error}")
    if stat.S_ISLNK(root_metadata.st_mode) or not stat.S_ISDIR(root_metadata.st_mode):
        fail(f"{label} {configured} must be a non-symlink directory")
    canonical = configured.resolve(strict=True)
    mode = stat.S_IMODE(root_metadata.st_mode)
    if root_metadata.st_uid != 0 or mode & 0o022:
        fail(f"{label} {configured} must be root-owned and not group/world writable")
    _trusted_ancestry(canonical, label)

    digest = hashlib.sha256()
    entry_count = 1
    total_bytes = 0
    _manifest_record(
        digest,
        ".",
        ["dir", str(mode), str(root_metadata.st_uid), str(root_metadata.st_gid)],
    )
    pending = [canonical]
    while pending:
        directory = pending.pop()
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as error:
            fail(f"cannot enumerate trusted runtime tree {directory}: {error}")
        for entry in entries:
            path = Path(entry.path)
            relative = path.relative_to(canonical).as_posix()
            try:
                metadata = path.lstat()
            except OSError as error:
                fail(f"cannot inspect trusted runtime entry {path}: {error}")
            entry_count += 1
            entry_mode = stat.S_IMODE(metadata.st_mode)
            if metadata.st_uid != 0:
                fail(f"trusted runtime entry {path} must be root-owned")
            if stat.S_ISDIR(metadata.st_mode):
                if entry_mode & 0o022:
                    fail(f"trusted runtime directory {path} must not be group/world writable")
                _manifest_record(
                    digest,
                    relative,
                    ["dir", str(entry_mode), str(metadata.st_uid), str(metadata.st_gid)],
                )
                pending.append(path)
            elif stat.S_ISREG(metadata.st_mode):
                if entry_mode & 0o022:
                    fail(f"trusted runtime file {path} must not be group/world writable")
                content_digest = sha256_file(path)
                total_bytes += metadata.st_size
                _manifest_record(
                    digest,
                    relative,
                    [
                        "file",
                        str(entry_mode),
                        str(metadata.st_uid),
                        str(metadata.st_gid),
                        str(metadata.st_size),
                        content_digest,
                    ],
                )
            elif stat.S_ISLNK(metadata.st_mode):
                try:
                    target = os.readlink(path)
                    resolved = path.resolve(strict=True)
                    resolved.relative_to(canonical)
                except (OSError, ValueError) as error:
                    fail(f"trusted runtime symlink {path} must resolve inside its root: {error}")
                _manifest_record(
                    digest,
                    relative,
                    ["symlink", str(metadata.st_uid), str(metadata.st_gid), target],
                )
            else:
                fail(f"trusted runtime entry {path} has an unsupported special-file type")

    base = {
        "configuredPath": str(configured),
        "canonicalPath": str(canonical),
        "device": root_metadata.st_dev,
        "inode": root_metadata.st_ino,
        "uid": root_metadata.st_uid,
        "gid": root_metadata.st_gid,
        "mode": mode,
    }
    return {
        **base,
        "identitySha256": _identity_digest(base),
        "treeSha256": digest.hexdigest(),
        "entryCount": entry_count,
        "totalBytes": total_bytes,
    }


def directory_identity(
    path_text: str,
    label: str,
    *,
    expected_uid: int | None = None,
    expected_gid: int | None = None,
) -> dict[str, Any]:
    configured = Path(_require_absolute_path(path_text, label))
    try:
        metadata = configured.lstat()
    except OSError as error:
        fail(f"{label} {configured} cannot be inspected: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        fail(f"{label} {configured} must be a non-symlink directory")
    canonical = configured.resolve(strict=True)
    if expected_uid is not None and metadata.st_uid != expected_uid:
        fail(f"{label} {configured} must be owned by uid {expected_uid}")
    if expected_gid is not None and metadata.st_gid != expected_gid:
        fail(f"{label} {configured} must be owned by gid {expected_gid}")
    mode = stat.S_IMODE(metadata.st_mode)
    base = {
        "configuredPath": str(configured),
        "canonicalPath": str(canonical),
        "device": metadata.st_dev,
        "inode": metadata.st_ino,
        "uid": metadata.st_uid,
        "gid": metadata.st_gid,
        "mode": mode,
    }
    return {**base, "identitySha256": _identity_digest(base)}


def _tool_identity(
    spec: dict[str, Any], label: str, *, executable: bool
) -> dict[str, Any]:
    identity = secure_file_identity(spec["path"], label, executable=executable)
    if identity["contentSha256"] != spec["sha256"]:
        fail(f"{label} content does not match the operator-pinned SHA-256")
    return identity


def _read_words(path: Path, label: str) -> list[str]:
    try:
        return sorted(set(path.read_text(encoding="utf-8").split()))
    except OSError as error:
        fail(f"cannot read {label} {path}: {error}")


def _unescape_mount_path(value: str) -> str:
    return (
        value.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
    )


def find_cgroup2_mount(cgroup_root: Path) -> Path:
    try:
        lines = Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"cannot inspect /proc/self/mountinfo: {error}")
    candidates: list[Path] = []
    for line in lines:
        if " - cgroup2 " not in line:
            continue
        fields = line.split()
        if len(fields) < 6:
            continue
        mount_point = Path(_unescape_mount_path(fields[4]))
        try:
            cgroup_root.relative_to(mount_point)
        except ValueError:
            continue
        candidates.append(mount_point)
    if not candidates:
        fail(f"{cgroup_root} is not below a visible cgroup-v2 mount")
    return max(candidates, key=lambda item: len(str(item)))


def current_cgroup_path(mount_point: Path) -> Path:
    try:
        lines = Path("/proc/self/cgroup").read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"cannot inspect /proc/self/cgroup: {error}")
    unified = [line.split("::", 1)[1] for line in lines if line.startswith("0::")]
    if len(unified) != 1:
        fail("/proc/self/cgroup must contain exactly one unified cgroup-v2 entry")
    return (mount_point / unified[0].lstrip("/")).resolve(strict=False)


def inspect_cgroup_boundary(
    cgroup_root_text: str, *, expected_uid: int, expected_gid: int
) -> dict[str, Any]:
    configured_root = Path(cgroup_root_text)
    try:
        metadata = configured_root.lstat()
    except OSError as error:
        fail(f"delegated cgroup root {configured_root} cannot be inspected: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        fail("delegated cgroup root must be a non-symlink directory")
    root = configured_root.resolve(strict=True)
    mount_point = find_cgroup2_mount(root)
    if root == mount_point:
        fail("the cgroup-v2 mount root cannot be delegated")
    current = current_cgroup_path(mount_point)
    try:
        current.relative_to(root)
    except ValueError:
        pass
    else:
        fail(f"canary process cgroup {current} is inside delegated root {root}")
    identity = directory_identity(
        str(root),
        "delegated cgroup root",
        expected_uid=expected_uid,
        expected_gid=expected_gid,
    )
    if identity["mode"] & 0o022:
        fail("delegated cgroup root must not be group/world writable")
    if not os.access(root, os.W_OK | os.X_OK):
        fail("delegated cgroup root must be writable/traversable by the canary identity")
    controllers = _read_words(root / "cgroup.controllers", "cgroup controllers")
    subtree = _read_words(root / "cgroup.subtree_control", "cgroup subtree control")
    for label, values in (("controllers", controllers), ("subtree control", subtree)):
        missing = sorted(REQUIRED_CONTROLLERS - set(values))
        if missing:
            fail(f"delegated cgroup root {label} missing: {', '.join(missing)}")
    if not (root / "cgroup.kill").exists() or not os.access(root / "cgroup.kill", os.W_OK):
        fail("delegated cgroup root must expose writable cgroup.kill")
    for control in ("cgroup.procs", "cgroup.threads"):
        try:
            content = (root / control).read_text(encoding="utf-8").strip()
        except OSError as error:
            fail(f"cannot inspect delegated root {control}: {error}")
        if content:
            fail(f"delegated root observation found processes in {control}")
    try:
        children = sorted(
            entry.name for entry in os.scandir(root) if entry.is_dir(follow_symlinks=False)
        )
    except OSError as error:
        fail(f"cannot enumerate delegated cgroup root {root}: {error}")
    if children:
        fail("delegated root observation found child cgroups: " + ", ".join(children))
    namespace = os.stat("/proc/self/ns/cgroup")
    return {
        "path": str(root),
        "device": identity["device"],
        "inode": identity["inode"],
        "uid": identity["uid"],
        "gid": identity["gid"],
        "mountPoint": str(mount_point),
        "controllers": controllers,
        "subtreeControl": subtree,
        "killWritable": True,
        "empty": True,
        "unknownChildren": [],
        "currentCgroupPath": str(current),
        "cgroupNamespace": {"device": namespace.st_dev, "inode": namespace.st_ino},
    }


def _exact_int_object(
    value: Any, label: str, fields: tuple[str, ...]
) -> dict[str, int]:
    obj = _require_object(value, label, set(fields))
    return {
        field: _require_int(obj[field], f"{label}.{field}", minimum=0)
        for field in fields
    }


def inspect_host_config(value: Any, contract: dict[str, Any]) -> dict[str, Any]:
    config = _require_object(value, "host config")
    proc_isolation = _require_object(
        config.get("proc_isolation"), "host config.proc_isolation"
    )
    required_isolation_keys = {
        "provider",
        "launcher",
        "runtime_roots",
        "limits",
        "cgroup_root",
        "cgroup_limits",
    }
    if set(proc_isolation) != required_isolation_keys:
        missing = sorted(required_isolation_keys - set(proc_isolation))
        unknown = sorted(set(proc_isolation) - required_isolation_keys)
        if missing:
            fail(
                "host config.proc_isolation is missing explicit fields: "
                + ", ".join(missing)
            )
        fail("host config.proc_isolation contains unknown fields: " + ", ".join(unknown))
    if _require_string(
        proc_isolation["provider"], "host config.proc_isolation.provider"
    ) != "linux_hardened_v1":
        fail("host config.proc_isolation.provider must be linux_hardened_v1")
    launcher = _require_absolute_path(
        proc_isolation["launcher"], "host config.proc_isolation.launcher"
    )
    runtime_roots = _require_unique_absolute_paths(
        proc_isolation["runtime_roots"], "host config.proc_isolation.runtime_roots"
    )
    cgroup_root = _require_absolute_path(
        proc_isolation["cgroup_root"], "host config.proc_isolation.cgroup_root"
    )
    rlimits = _exact_int_object(
        proc_isolation["limits"],
        "host config.proc_isolation.limits",
        ("address_space_bytes", "process_count", "open_files"),
    )
    cgroup_limits = _exact_int_object(
        proc_isolation["cgroup_limits"],
        "host config.proc_isolation.cgroup_limits",
        (
            "memory_max_bytes",
            "memory_swap_max_bytes",
            "pids_max",
            "cpu_quota_micros",
            "cpu_period_micros",
        ),
    )
    if any(value <= 0 for value in rlimits.values()):
        fail("host config proc isolation rlimits must be positive")
    for field in (
        "memory_max_bytes",
        "pids_max",
        "cpu_quota_micros",
        "cpu_period_micros",
    ):
        if cgroup_limits[field] <= 0:
            fail(f"host config cgroup {field} must be positive")

    linked = _require_array(
        config.get("linked_folders"), "host config.linked_folders", nonempty=True
    )
    linked_entries: list[dict[str, str]] = []
    for index, item in enumerate(linked):
        entry = _require_object(item, f"host config.linked_folders[{index}]")
        path = _require_absolute_path(
            entry.get("path"), f"host config.linked_folders[{index}].path"
        )
        mode = _require_string(
            entry.get("mode"), f"host config.linked_folders[{index}].mode"
        )
        if mode not in {"ro", "rw"}:
            fail(f"host config.linked_folders[{index}].mode must be ro or rw")
        linked_entries.append({"path": path, "mode": mode})
    if len({entry["path"] for entry in linked_entries}) != len(linked_entries):
        fail("host config linked folder paths must be unique")

    expected = contract["isolation"]
    for label, actual, wanted in (
        ("launcher", launcher, expected["launcher"]),
        ("cgroup root", cgroup_root, expected["cgroupRoot"]),
        ("runtime roots", runtime_roots, expected["runtimeRoots"]),
        ("linked roots and modes", linked_entries, expected["linkedRoots"]),
    ):
        if actual != wanted:
            fail(f"host config {label} does not exactly match the deployment contract")
    return {
        "provider": "linux_hardened_v1",
        "launcher": launcher,
        "runtimeRoots": runtime_roots,
        "cgroupRoot": cgroup_root,
        "linkedRoots": linked_entries,
        "rlimits": {
            "addressSpaceBytes": rlimits["address_space_bytes"],
            "processCount": rlimits["process_count"],
            "openFiles": rlimits["open_files"],
        },
        "cgroupLimits": {
            "memoryMaxBytes": cgroup_limits["memory_max_bytes"],
            "memorySwapMaxBytes": cgroup_limits["memory_swap_max_bytes"],
            "pidsMax": cgroup_limits["pids_max"],
            "cpuQuotaMicros": cgroup_limits["cpu_quota_micros"],
            "cpuPeriodMicros": cgroup_limits["cpu_period_micros"],
        },
    }


def verify_disposable_service_fixture(path: Path, service: dict[str, Any]) -> None:
    try:
        lines = [
            line.strip()
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith(("#", ";"))
        ]
    except (OSError, UnicodeDecodeError) as error:
        fail(f"cannot inspect disposable service-manager fixture: {error}")
    if any(line.startswith("EnvironmentFile=") for line in lines):
        fail("disposable service-manager fixture must not load an EnvironmentFile")
    required = [
        f"User={service['expectedUid']}",
        f"Group={service['expectedGid']}",
        f"Environment=TM_HOST_CONFIG={service['hostConfigPath']}",
        f"Environment=TM_SERVER_ROLE={service['role']}",
        f"ExecStart={service['serverBinaryPath']}",
    ]
    for expected in required:
        if lines.count(expected) != 1:
            fail(
                "disposable service-manager fixture must contain exactly one "
                f"{expected!r} directive"
            )
    for prefix in ("User=", "Group=", "ExecStart="):
        if len([line for line in lines if line.startswith(prefix)]) != 1:
            fail(f"disposable service-manager fixture contains conflicting {prefix} directives")
    allowed_environment = {
        "Environment=TM_HOST_CONFIG=" + service["hostConfigPath"],
        "Environment=TM_SERVER_ROLE=" + service["role"],
    }
    actual_environment = {line for line in lines if line.startswith("Environment=")}
    if actual_environment != allowed_environment:
        fail("disposable service-manager fixture environment is not the exact bounded set")


def source_hashes(repo_root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for relative in SOURCE_PATHS:
        path = repo_root / relative
        try:
            metadata = path.lstat()
        except OSError:
            fail(f"required M4 source file is missing: {relative}")
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            fail(f"required M4 source file must be regular and non-symlink: {relative}")
        result[relative] = sha256_file(path)
    return result


def _git_environment() -> dict[str, str]:
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": "/",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_OPTIONAL_LOCKS": "0",
        "LC_ALL": "C",
    }


def git_evidence(
    repo_root: Path, git_path: str, expected_revision: str
) -> dict[str, Any]:
    base = [
        git_path,
        "--no-optional-locks",
        "-c",
        f"safe.directory={repo_root}",
        "-c",
        "core.hooksPath=/dev/null",
    ]

    def run(*args: str) -> str:
        try:
            result = subprocess.run(
                [*base, *args],
                cwd=repo_root,
                env=_git_environment(),
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            fail(f"cannot execute pinned Git for M4 evidence: {error}")
        if result.returncode != 0:
            detail = result.stderr.strip() or f"exit status {result.returncode}"
            fail(f"cannot record Git evidence: {detail}")
        return result.stdout.rstrip("\n")

    revision = run("rev-parse", "HEAD").strip()
    if revision != expected_revision:
        fail(
            "checked-out Git revision does not equal the operator-authored expected revision"
        )
    status = run("status", "--porcelain=v1", "--untracked-files=all")
    if status:
        fail("M4 evidence requires a clean Git tree; dirty paths: " + " | ".join(status.splitlines()))
    return {"revision": revision, "dirty": False, "statusShort": []}


def _state_binding_digest(state: dict[str, Any]) -> str:
    unbound = {key: value for key, value in state.items() if key != "stateBindingSha256"}
    return _canonical_digest(unbound)


def _bind_state(state: dict[str, Any]) -> dict[str, Any]:
    result = {**state}
    result["stateBindingSha256"] = _state_binding_digest(result)
    return result


def _validate_state(value: Any) -> dict[str, Any]:
    state = _require_object(value, "internal state")
    if state.get("stateVersion") != 2:
        fail("internal state has an unsupported version")
    binding = _validate_sha256(
        state.get("stateBindingSha256"), "internal state.stateBindingSha256"
    )
    if binding != _state_binding_digest(state):
        fail("internal state binding does not match its contents")
    return state


def _write_internal_state(path: Path, value: dict[str, Any]) -> None:
    state = _bind_state(value)
    encoded = json.dumps(state, sort_keys=True, separators=(",", ":")).encode("utf-8")
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except OSError as error:
        fail(f"cannot create internal state {path}: {error}")
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
    except Exception:
        try:
            path.unlink()
        except OSError:
            pass
        raise


def _parse_decimal(value: str, label: str, *, minimum: int) -> int:
    if re.fullmatch(r"[1-9][0-9]*", value or "") is None:
        fail(f"{label} must be an operator-authored decimal integer")
    parsed = int(value, 10)
    if parsed < minimum:
        fail(f"{label} must be at least {minimum}")
    return parsed


def _utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds").replace(
        "+00:00", "Z"
    )


def _verify_state_directory(path: Path, uid: int, gid: int) -> None:
    if not path.is_absolute() or not path.name.startswith("tm-m4-acceptance-"):
        fail("state directory must be absolute and named tm-m4-acceptance-*")
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect state directory {path}: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        fail("state directory must be a non-symlink directory")
    if metadata.st_uid != uid or metadata.st_gid != gid:
        fail("state directory ownership must match the canary identity")
    if stat.S_IMODE(metadata.st_mode) & 0o077:
        fail("state directory must not be accessible by group or other users")


def _verify_invoked_tool(
    actual_path: str, expected: dict[str, Any], label: str
) -> None:
    try:
        actual = Path(actual_path).resolve(strict=True)
        wanted = Path(expected["canonicalPath"]).resolve(strict=True)
    except OSError as error:
        fail(f"cannot resolve invoked {label}: {error}")
    if actual != wanted:
        fail(f"invoked {label} does not match the contract-pinned executable")


def build_preflight_state(args: argparse.Namespace) -> dict[str, Any]:
    if platform.system() != "Linux":
        fail("M4 live preflight requires Linux")
    expected_uid = _parse_decimal(args.expected_uid, "--expected-uid", minimum=1)
    expected_gid = _parse_decimal(args.expected_gid, "--expected-gid", minimum=1)
    expected_arch = _require_string(args.expected_arch, "--expected-arch")
    if expected_arch not in SUPPORTED_ARCHITECTURES:
        fail("--expected-arch must be aarch64 or x86_64")
    if os.geteuid() != expected_uid or os.getegid() != expected_gid:
        fail(
            f"canary identity mismatch: expected {expected_uid}:{expected_gid}, "
            f"got {os.geteuid()}:{os.getegid()}"
        )
    if platform.machine() != expected_arch:
        fail(f"architecture mismatch: expected {expected_arch}, got {platform.machine()}")

    contract_path = Path(_require_absolute_path(args.contract, "--contract"))
    host_config_path = Path(_require_absolute_path(args.host_config, "--host-config"))
    state_output = Path(_require_absolute_path(args.state_output, "--state-output"))
    _verify_state_directory(state_output.parent, expected_uid, expected_gid)
    if state_output.exists() or state_output.is_symlink():
        fail("internal state output already exists")

    contract_identity = secure_file_identity(str(contract_path), "deployment contract")
    host_config_identity = secure_file_identity(str(host_config_path), "host config")
    contract = validate_contract(load_json(contract_path), live=True)
    service = contract["service"]
    if (service["expectedUid"], service["expectedGid"]) != (
        expected_uid,
        expected_gid,
    ):
        fail("operator UID/GID do not exactly match the deployment contract")
    if service["expectedArchitecture"] != expected_arch:
        fail("operator architecture does not exactly match the deployment contract")
    if service["hostConfigPath"] != str(host_config_path):
        fail("operator TM_HOST_CONFIG path does not exactly match the contract")

    host_config = inspect_host_config(load_json(host_config_path), contract)
    tool_identities: dict[str, dict[str, Any]] = {}
    for name in sorted(TOOL_NAMES):
        tool_identities[name] = _tool_identity(
            contract["tools"][name],
            f"contract-pinned {name}",
            executable=name != "acceptanceProgram",
        )
    for argument, name in (
        (args.python, "python"),
        (args.bash, "bash"),
        (args.wrapper, "wrapper"),
        (args.acceptance_program, "acceptanceProgram"),
    ):
        if _require_absolute_path(argument, f"--{name}") != contract["tools"][name]["path"]:
            fail(f"invoked {name} path does not equal the contract")
    _verify_invoked_tool(sys.executable, tool_identities["python"], "Python")
    _verify_invoked_tool(__file__, tool_identities["acceptanceProgram"], "acceptance program")
    try:
        parent_executable = os.readlink(f"/proc/{os.getppid()}/exe")
    except OSError as error:
        fail(f"cannot inspect parent Bash executable: {error}")
    _verify_invoked_tool(parent_executable, tool_identities["bash"], "Bash")

    manager = secure_file_identity(
        service["serviceManagerConfigPath"], "service-manager fixture"
    )
    server = secure_file_identity(
        service["serverBinaryPath"], "tm-server fixture binary", executable=True
    )
    if server["contentSha256"] != _runtime_artifact_sha256(
        service["runtimeArtifactIdentity"]
    ):
        fail("contract runtime artifact does not match the exact server-binary content")
    verify_disposable_service_fixture(Path(service["serviceManagerConfigPath"]), service)
    launcher = secure_file_identity(
        host_config["launcher"], "proc isolation launcher", executable=True
    )
    runtime_roots = [
        tree_identity(path, "proc isolation runtime root")
        for path in host_config["runtimeRoots"]
    ]
    linked_roots: list[dict[str, Any]] = []
    for entry in host_config["linkedRoots"]:
        identity = directory_identity(entry["path"], "linked project root")
        if not os.access(entry["path"], os.R_OK | os.X_OK):
            fail(f"linked project root {entry['path']} is not readable/traversable")
        if entry["mode"] == "rw" and not os.access(entry["path"], os.W_OK):
            fail(f"rw linked project root {entry['path']} is not writable")
        linked_roots.append({"mode": entry["mode"], "identity": identity})

    boundary = inspect_cgroup_boundary(
        host_config["cgroupRoot"],
        expected_uid=expected_uid,
        expected_gid=expected_gid,
    )
    repo_root = Path(contract["source"]["repositoryRoot"])
    if not repo_root.is_dir() or repo_root.is_symlink():
        fail("contract source repositoryRoot must be an existing non-symlink directory")
    git = git_evidence(
        repo_root,
        tool_identities["git"]["canonicalPath"],
        contract["source"]["expectedRevision"],
    )
    return {
        "stateVersion": 2,
        "startedAt": _utc_now(),
        "startedEpochNs": time.time_ns(),
        "repoRoot": str(repo_root),
        "contractData": contract,
        "contract": contract_identity,
        "hostConfig": {"identity": host_config_identity, **host_config},
        "service": {
            "name": service["name"],
            "role": service["role"],
            "uid": expected_uid,
            "gid": expected_gid,
            "architecture": expected_arch,
            "kernel": platform.release(),
            "runtimeArtifactIdentity": service["runtimeArtifactIdentity"],
            "environment": service["expectedEnvironment"],
            "semantics": "disposable_fixture_only",
            "serverBinary": server,
            "serviceManagerConfig": manager,
        },
        "tools": tool_identities,
        "isolation": {
            "provider": "linux_hardened_v1",
            "launcher": launcher,
            "runtimeRoots": runtime_roots,
            "linkedRoots": linked_roots,
            "cgroupBefore": boundary,
        },
        "git": git,
        "sourceSha256": source_hashes(repo_root),
    }


def _identity_unchanged(
    before: dict[str, Any], after: dict[str, Any], label: str
) -> None:
    if before != after:
        fail(f"{label} changed while the M4 canary was running")


def sanitized_canary_environment(state: dict[str, Any]) -> dict[str, str]:
    state = _validate_state(state)
    configured = state["contractData"]["execution"]["environment"]
    environment = {key: configured[key] for key in sorted(EXECUTION_ENV_KEYS)}
    environment["TM_HOST_CONFIG"] = state["hostConfig"]["identity"]["configuredPath"]
    environment["TM_LINUX_HARDENED_TESTS"] = "1"
    if set(environment) != EXECUTION_ENV_KEYS | {
        "TM_HOST_CONFIG",
        "TM_LINUX_HARDENED_TESTS",
    }:
        fail("internal canary environment escaped its exact allowlist")
    return environment


def _terminate_process_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=2)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        fail("canary process group survived SIGKILL")


def run_bounded_canary(state: dict[str, Any], output_path: Path) -> None:
    state = _validate_state(state)
    if not output_path.is_absolute():
        fail("canary output path must be absolute")
    if output_path.parent != Path(state["contractData"]["execution"]["environment"]["TMPDIR"]):
        fail("canary output must stay in the contract-pinned private TMPDIR")
    _verify_state_directory(output_path.parent, state["service"]["uid"], state["service"]["gid"])
    if output_path.exists() or output_path.is_symlink():
        fail("canary output already exists")
    canary_before = state["tools"]["canaryExecutable"]
    _identity_unchanged(
        canary_before,
        secure_file_identity(
            canary_before["configuredPath"], "prebuilt canary", executable=True
        ),
        "prebuilt canary",
    )
    arguments = [canary_before["canonicalPath"], *CANARY_ARGUMENTS]
    environment = sanitized_canary_environment(state)
    timeout_seconds = state["contractData"]["execution"]["timeoutSeconds"]
    try:
        output_fd = os.open(
            output_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600
        )
    except OSError as error:
        fail(f"cannot create private canary output: {error}")
    process: subprocess.Popen[bytes] | None = None
    succeeded = False
    try:
        try:
            process = subprocess.Popen(
                arguments,
                cwd=state["repoRoot"],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                close_fds=True,
                start_new_session=True,
            )
        except OSError as error:
            fail(f"cannot execute the pinned prebuilt canary: {error}")
        assert process.stdout is not None
        stream_fd = process.stdout.fileno()
        os.set_blocking(stream_fd, False)
        selector = selectors.DefaultSelector()
        selector.register(stream_fd, selectors.EVENT_READ)
        deadline = time.monotonic() + timeout_seconds
        total = 0
        eof = False
        while not eof or process.poll() is None:
            if time.monotonic() >= deadline:
                _terminate_process_group(process)
                fail(f"prebuilt canary exceeded the {timeout_seconds}s wall-time bound")
            events = selector.select(timeout=0.1)
            for key, _ in events:
                try:
                    chunk = os.read(key.fd, 65536)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(key.fd)
                    eof = True
                    continue
                total += len(chunk)
                if total > MAX_CANARY_OUTPUT_BYTES:
                    _terminate_process_group(process)
                    fail(
                        f"prebuilt canary exceeded the {MAX_CANARY_OUTPUT_BYTES}-byte output bound"
                    )
                os.write(output_fd, chunk)
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
            if process.poll() is not None and not events and eof:
                break
        return_code = process.wait(timeout=1)
        os.fsync(output_fd)
        if return_code != 0:
            fail(f"prebuilt canary exited with status {return_code}")
        succeeded = True
    finally:
        if process is not None and process.poll() is None:
            _terminate_process_group(process)
        os.close(output_fd)
        if not succeeded:
            try:
                output_path.unlink()
            except OSError:
                pass


def _tool_identities_after(state: dict[str, Any]) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for name, identity in state["tools"].items():
        result[name] = secure_file_identity(
            identity["configuredPath"],
            f"contract-pinned {name}",
            executable=name != "acceptanceProgram",
        )
    return result


def finalize_report(state: dict[str, Any], canary_output_path: Path) -> dict[str, Any]:
    state = _validate_state(state)
    try:
        output = canary_output_path.read_bytes()
    except OSError as error:
        fail(f"cannot read private canary output: {error}")
    if not output or len(output) > MAX_CANARY_OUTPUT_BYTES:
        fail("private canary output is empty or exceeds the bound")
    try:
        output_text = output.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"private canary output is not UTF-8: {error}")
    if re.search(r"test result: ok\. 1 passed; 0 failed;", output_text) is None:
        fail("exact hardened canary success summary was not found")

    contract_path = state["contract"]["configuredPath"]
    host_config_path = state["hostConfig"]["identity"]["configuredPath"]
    service = state["service"]
    host_config = state["hostConfig"]
    isolation = state["isolation"]
    _identity_unchanged(
        state["contract"],
        secure_file_identity(contract_path, "deployment contract"),
        "deployment contract",
    )
    _identity_unchanged(
        host_config["identity"],
        secure_file_identity(host_config_path, "host config"),
        "host config",
    )
    _identity_unchanged(
        service["serviceManagerConfig"],
        secure_file_identity(
            service["serviceManagerConfig"]["configuredPath"],
            "service-manager fixture",
        ),
        "service-manager fixture",
    )
    _identity_unchanged(
        service["serverBinary"],
        secure_file_identity(
            service["serverBinary"]["configuredPath"],
            "tm-server fixture binary",
            executable=True,
        ),
        "tm-server fixture binary",
    )
    _identity_unchanged(
        isolation["launcher"],
        secure_file_identity(
            isolation["launcher"]["configuredPath"],
            "proc isolation launcher",
            executable=True,
        ),
        "proc isolation launcher",
    )
    tools_after = _tool_identities_after(state)
    _identity_unchanged(state["tools"], tools_after, "contract-pinned tools")
    runtime_after = [
        tree_identity(item["configuredPath"], "proc isolation runtime root")
        for item in isolation["runtimeRoots"]
    ]
    linked_after = [
        {
            "mode": item["mode"],
            "identity": directory_identity(
                item["identity"]["configuredPath"], "linked project root"
            ),
        }
        for item in isolation["linkedRoots"]
    ]
    _identity_unchanged(isolation["runtimeRoots"], runtime_after, "runtime contents")
    _identity_unchanged(isolation["linkedRoots"], linked_after, "linked roots")
    verify_disposable_service_fixture(
        Path(service["serviceManagerConfig"]["configuredPath"]),
        state["contractData"]["service"],
    )

    boundary_after = inspect_cgroup_boundary(
        host_config["cgroupRoot"],
        expected_uid=service["uid"],
        expected_gid=service["gid"],
    )
    boundary_before = isolation["cgroupBefore"]
    for field in (
        "path",
        "device",
        "inode",
        "uid",
        "gid",
        "mountPoint",
        "controllers",
        "subtreeControl",
        "currentCgroupPath",
        "cgroupNamespace",
    ):
        if boundary_before[field] != boundary_after[field]:
            fail(f"delegated cgroup observation field {field} changed during the canary")

    repo_root = Path(state["repoRoot"])
    current_sources = source_hashes(repo_root)
    if current_sources != state["sourceSha256"]:
        fail("M4 source files changed while the canary was running")
    current_git = git_evidence(
        repo_root,
        state["tools"]["git"]["canonicalPath"],
        state["contractData"]["source"]["expectedRevision"],
    )
    if current_git != state["git"]:
        fail("Git evidence changed while the canary was running")

    environment = sanitized_canary_environment(state)
    proven = [
        "The pinned prebuilt exact canary passed under a hostile-workload and trusted-host-kernel boundary.",
        "The clean operator-pinned revision, contract, host config, runtime contents, tools, and fixture identities remained stable.",
        "The delegated cgroup root was observed empty before and after, with cpu, memory, and pids controllers enabled.",
        "The canary ran with a bounded wall time, bounded streamed output, and an exact non-secret environment allowlist.",
    ]
    not_proven = [
        "Production service deployment semantics, supervisor activation, or production-host acceptance.",
        "Production cgroup exclusivity; no operator/supervisor lease or unique binding is present in acceptance-kit v1.",
        "Production workload sizing, representative memory.peak and pids.peak metrics, or selected headroom.",
        "Hostile host-kernel containment; linux_hardened_v1 assumes a trusted host kernel.",
        "microVM isolation or separate-kernel assurance.",
        "Native x86_64 acceptance until the manual workflow actually retains a validated report.",
        "Full M4 milestone closeout from this disposable report alone.",
    ]
    cgroup_report = {
        "path": boundary_before["path"],
        "device": boundary_before["device"],
        "inode": boundary_before["inode"],
        "uid": boundary_before["uid"],
        "gid": boundary_before["gid"],
        "mountPoint": boundary_before["mountPoint"],
        "controllers": boundary_before["controllers"],
        "subtreeControl": boundary_before["subtreeControl"],
        "killWritable": True,
        "emptyBefore": True,
        "emptyAfter": True,
        "unknownChildrenBefore": [],
        "unknownChildrenAfter": [],
        "exclusivity": state["contractData"]["isolation"]["exclusivity"],
    }
    report: dict[str, Any] = {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "reportType": REPORT_TYPE,
        "status": "passed",
        "recordedAt": _utc_now(),
        "evidenceClass": EVIDENCE_CLASS,
        "threatModel": state["contractData"]["threatModel"],
        "microvm": state["contractData"]["microvm"],
        "workloadSizing": state["contractData"]["workloadSizing"],
        "contract": {
            "identity": state["contract"],
            "schemaVersion": CONTRACT_SCHEMA_VERSION,
            "canonicalSha256": _canonical_digest(state["contractData"]),
        },
        "hostConfig": {
            "identity": host_config["identity"],
            "provider": host_config["provider"],
            "rlimits": host_config["rlimits"],
            "cgroupLimits": host_config["cgroupLimits"],
        },
        "service": {
            **service,
            "currentCgroupPath": boundary_before["currentCgroupPath"],
            "cgroupNamespace": boundary_before["cgroupNamespace"],
        },
        "tools": state["tools"],
        "isolation": {
            "provider": isolation["provider"],
            "launcher": isolation["launcher"],
            "runtimeRoots": isolation["runtimeRoots"],
            "linkedRoots": isolation["linkedRoots"],
            "cgroup": cgroup_report,
        },
        "canary": {
            "arguments": CANARY_ARGUMENTS,
            "status": "passed",
            "durationMs": max(
                0, (time.time_ns() - state["startedEpochNs"]) // 1_000_000
            ),
            "timeoutSeconds": state["contractData"]["execution"]["timeoutSeconds"],
            "outputBytes": len(output),
            "outputSha256": hashlib.sha256(output).hexdigest(),
            "outputRetained": False,
            "successSummaryMatched": True,
            "environmentKeys": sorted(environment),
            "environmentSha256": _canonical_digest(environment),
        },
        "git": state["git"],
        "sourceSha256": state["sourceSha256"],
        "assertions": {key: "passed" for key in sorted(REPORT_ASSERTION_KEYS)},
        "scopeBoundary": {"proven": proven, "notProven": not_proven},
    }
    report["bindingSha256"] = _report_binding_digest(report)
    validate_report(report)
    return report


def validate_report_expectations(
    report: dict[str, Any],
    *,
    expected_contract_path: Path,
    expected_host_config_path: Path,
    expected_revision: str,
    expected_runtime_artifact: str,
) -> dict[str, Any]:
    validate_report(report)
    if re.fullmatch(r"[0-9a-f]{40}", expected_revision) is None:
        fail("--expected-revision must be a full lowercase Git revision")
    expected_runtime_artifact = _require_runtime_artifact_digest(
        expected_runtime_artifact, "--expected-runtime-artifact"
    )
    contract = validate_contract(load_json(expected_contract_path), live=True)
    host_config_value = load_json(expected_host_config_path)
    inspected_config = inspect_host_config(host_config_value, contract)
    if report["contract"]["identity"]["contentSha256"] != sha256_file(
        expected_contract_path
    ):
        fail("report does not match the externally supplied expected contract")
    if report["contract"]["canonicalSha256"] != _canonical_digest(contract):
        fail("report canonical contract binding does not match the expected contract")
    if report["hostConfig"]["identity"]["contentSha256"] != sha256_file(
        expected_host_config_path
    ):
        fail("report does not match the externally supplied expected host config")
    if contract["source"]["expectedRevision"] != expected_revision:
        fail("expected revision does not match the deployment contract")
    if report["git"]["revision"] != expected_revision:
        fail("report Git revision does not match the externally supplied expectation")
    if contract["service"]["runtimeArtifactIdentity"] != expected_runtime_artifact:
        fail("expected runtime artifact does not match the deployment contract")
    if report["service"]["runtimeArtifactIdentity"] != expected_runtime_artifact:
        fail("report runtime artifact does not match the external expectation")

    service = contract["service"]
    report_service = report["service"]
    for report_field, contract_field in (
        ("name", "name"),
        ("role", "role"),
        ("uid", "expectedUid"),
        ("gid", "expectedGid"),
        ("architecture", "expectedArchitecture"),
        ("environment", "expectedEnvironment"),
    ):
        if report_service[report_field] != service[contract_field]:
            fail(f"report service {report_field} does not match the expected contract")
    for report_field, contract_field in (
        ("serverBinary", "serverBinaryPath"),
        ("serviceManagerConfig", "serviceManagerConfigPath"),
    ):
        if report_service[report_field]["configuredPath"] != service[contract_field]:
            fail(f"report {report_field} path does not match the expected contract")

    for field in ("provider", "rlimits", "cgroupLimits"):
        if report["hostConfig"][field] != inspected_config[field]:
            fail(f"report hostConfig.{field} does not match the expected config")
    if report["hostConfig"]["identity"]["configuredPath"] != service["hostConfigPath"]:
        fail("report host config configured path does not match the contract")
    if report["isolation"]["launcher"]["configuredPath"] != inspected_config["launcher"]:
        fail("report launcher does not match the expected host config")
    if [item["configuredPath"] for item in report["isolation"]["runtimeRoots"]] != inspected_config["runtimeRoots"]:
        fail("report runtime roots do not match the expected host config")
    report_linked = [
        {"path": item["identity"]["configuredPath"], "mode": item["mode"]}
        for item in report["isolation"]["linkedRoots"]
    ]
    if report_linked != inspected_config["linkedRoots"]:
        fail("report linked roots/modes do not match the expected host config")
    if report["isolation"]["cgroup"]["path"] != inspected_config["cgroupRoot"]:
        fail("report cgroup root does not match the expected host config")
    if report["isolation"]["cgroup"]["exclusivity"] != contract["isolation"]["exclusivity"]:
        fail("report cgroup non-exclusivity boundary does not match the contract")

    for name in sorted(TOOL_NAMES):
        expected_tool = contract["tools"][name]
        actual_tool = report["tools"][name]
        if actual_tool["configuredPath"] != expected_tool["path"]:
            fail(f"report tool {name} path does not match the expected contract")
        if actual_tool["contentSha256"] != expected_tool["sha256"]:
            fail(f"report tool {name} digest does not match the expected contract")
    if report["canary"]["timeoutSeconds"] != contract["execution"]["timeoutSeconds"]:
        fail("report timeout does not match the expected contract")
    expected_environment = {
        **contract["execution"]["environment"],
        "TM_HOST_CONFIG": service["hostConfigPath"],
        "TM_LINUX_HARDENED_TESTS": "1",
    }
    if report["canary"]["environmentSha256"] != _canonical_digest(
        expected_environment
    ):
        fail("report sanitized environment does not match the expected contract")
    return report


def _atomic_create_report(
    path: Path,
    value: dict[str, Any],
    *,
    post_link_validator: Callable[[dict[str, Any]], Any] | None = None,
) -> None:
    validate_report(value)
    if not path.is_absolute():
        fail("evidence output path must be absolute")
    if path.exists() or path.is_symlink():
        fail(f"refusing to overwrite existing evidence report {path}")
    parent = path.parent
    try:
        parent_metadata = parent.lstat()
    except OSError as error:
        fail(f"cannot inspect evidence output directory: {error}")
    if stat.S_ISLNK(parent_metadata.st_mode) or not stat.S_ISDIR(parent_metadata.st_mode):
        fail("evidence output directory must be a non-symlink directory")
    encoded = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")
    temporary: Path | None = None
    linked = False
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            prefix=f".{path.name}.",
            suffix=".tmp",
            dir=parent,
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o644)
        os.link(temporary, path)
        linked = True
        persisted = load_json(path)
        validate_report(persisted)
        if post_link_validator is not None:
            post_link_validator(persisted)
        directory_fd = os.open(parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except Exception as error:
        if linked:
            try:
                path.unlink()
            except OSError as cleanup_error:
                raise ValidationError(
                    "post-link report validation failed and the invalid report could not "
                    f"be removed: {cleanup_error}"
                ) from cleanup_error
        if isinstance(error, FileExistsError):
            fail(f"refusing to overwrite existing evidence report {path}")
        if isinstance(error, ValidationError):
            raise
        if isinstance(error, OSError):
            fail(f"cannot atomically create evidence report {path}: {error}")
        raise
    finally:
        if temporary is not None:
            try:
                temporary.unlink()
            except OSError:
                pass


def cleanup_state_directory(path: Path) -> None:
    _verify_state_directory(path, os.geteuid(), os.getegid())
    allowed = {"preflight.json", "canary-output.bin"}
    try:
        entries = list(os.scandir(path))
    except OSError as error:
        fail(f"cannot inspect private state directory: {error}")
    unknown = sorted(entry.name for entry in entries if entry.name not in allowed)
    if unknown:
        fail("refusing to clean state directory with unknown entries: " + ", ".join(unknown))
    for name in sorted(allowed):
        candidate = path / name
        if candidate.is_symlink() or candidate.is_dir():
            fail(f"refusing to clean unexpected state entry {candidate}")
        try:
            candidate.unlink(missing_ok=True)
        except OSError as error:
            fail(f"cannot remove private state entry {candidate}: {error}")
    try:
        path.rmdir()
    except OSError as error:
        fail(f"cannot remove private state directory {path}: {error}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate_contract_parser = subparsers.add_parser("validate-contract")
    validate_contract_parser.add_argument("path")

    validate_report_parser = subparsers.add_parser("validate-report")
    validate_report_parser.add_argument("path")
    validate_report_parser.add_argument("--expected-contract", required=True)
    validate_report_parser.add_argument("--expected-host-config", required=True)
    validate_report_parser.add_argument("--expected-revision", required=True)
    validate_report_parser.add_argument("--expected-runtime-artifact", required=True)

    preflight = subparsers.add_parser("preflight")
    preflight.add_argument("--contract", required=True)
    preflight.add_argument("--host-config", required=True)
    preflight.add_argument("--expected-uid", required=True)
    preflight.add_argument("--expected-gid", required=True)
    preflight.add_argument("--expected-arch", required=True)
    preflight.add_argument("--python", required=True)
    preflight.add_argument("--bash", required=True)
    preflight.add_argument("--wrapper", required=True)
    preflight.add_argument("--acceptance-program", required=True)
    preflight.add_argument("--state-output", required=True)

    run_canary = subparsers.add_parser("run-canary")
    run_canary.add_argument("--state", required=True)
    run_canary.add_argument("--output", required=True)

    finalize = subparsers.add_parser("finalize")
    finalize.add_argument("--state", required=True)
    finalize.add_argument("--canary-output", required=True)
    finalize.add_argument("--output", required=True)

    cleanup = subparsers.add_parser("cleanup")
    cleanup.add_argument("--state-directory", required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.command == "validate-contract":
        validate_contract(load_json(Path(args.path)))
        print(f"M4 disposable deployment contract valid: {args.path}")
        return 0
    if args.command == "validate-report":
        report = load_json(Path(args.path))
        validate_report_expectations(
            report,
            expected_contract_path=Path(args.expected_contract),
            expected_host_config_path=Path(args.expected_host_config),
            expected_revision=args.expected_revision,
            expected_runtime_artifact=args.expected_runtime_artifact,
        )
        print(f"M4 contract-bound disposable report valid: {args.path}")
        return 0
    if args.command == "preflight":
        state = build_preflight_state(args)
        _write_internal_state(Path(args.state_output), state)
        print("M4 disposable live preflight passed")
        return 0
    if args.command == "run-canary":
        state = _validate_state(load_json(Path(args.state)))
        run_bounded_canary(state, Path(args.output))
        print("M4 bounded prebuilt canary passed")
        return 0
    if args.command == "finalize":
        state = _validate_state(load_json(Path(args.state)))
        output_path = Path(args.output)
        report = finalize_report(state, Path(args.canary_output))
        contract_path = Path(state["contract"]["configuredPath"])
        host_config_path = Path(state["hostConfig"]["identity"]["configuredPath"])
        expected_revision = state["contractData"]["source"]["expectedRevision"]
        expected_artifact = state["contractData"]["service"][
            "runtimeArtifactIdentity"
        ]

        def validate_persisted(persisted: dict[str, Any]) -> None:
            validate_report_expectations(
                persisted,
                expected_contract_path=contract_path,
                expected_host_config_path=host_config_path,
                expected_revision=expected_revision,
                expected_runtime_artifact=expected_artifact,
            )

        _atomic_create_report(
            output_path, report, post_link_validator=validate_persisted
        )
        print(f"M4 disposable acceptance report: {output_path}")
        return 0
    if args.command == "cleanup":
        cleanup_state_directory(Path(args.state_directory))
        return 0
    fail(f"unsupported command {args.command}")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"m4 acceptance: {error}", file=sys.stderr)
        raise SystemExit(1)
