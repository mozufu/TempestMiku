from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = REPO_ROOT / "tools" / "m4_acceptance.py"
SPEC = importlib.util.spec_from_file_location("m4_acceptance", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
m4 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(m4)


def tool_specs() -> dict[str, object]:
    paths = {
        "python": "/usr/bin/python3.13",
        "git": "/usr/bin/git",
        "bash": "/bin/bash",
        "wrapper": "/opt/tempestmiku-m4-kit/m4-linux-hardened-canary.sh",
        "acceptanceProgram": "/opt/tempestmiku-m4-kit/m4_acceptance.py",
        "canaryExecutable": "/opt/tempestmiku-m4-kit/tm-host-canary",
    }
    return {
        name: {"path": path, "sha256": f"{index + 1:x}" * 64}
        for index, (name, path) in enumerate(paths.items())
    }


def contract() -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "evidenceClass": m4.EVIDENCE_CLASS,
        "threatModel": {
            "hostKernel": "trusted",
            "workload": "hostile",
            "hostileHostKernelClaimed": False,
        },
        "microvm": {
            "required": False,
            "selected": False,
            "provider": None,
            "evidencePath": None,
        },
        "service": {
            "name": "tempestmiku-m4-disposable",
            "role": "all",
            "expectedUid": 23017,
            "expectedGid": 23017,
            "expectedArchitecture": "x86_64",
            "hostConfigPath": "/etc/tempestmiku/m4-host.json",
            "serviceManagerConfigPath": "/etc/systemd/system/tempestmiku-m4.service",
            "serverBinaryPath": "/opt/tempestmiku/bin/tm-server",
            "runtimeArtifactIdentity": "tm-server@sha256:" + "2" * 64,
            "expectedEnvironment": {
                "TM_HOST_CONFIG": "/etc/tempestmiku/m4-host.json",
                "TM_SERVER_ROLE": "all",
            },
        },
        "isolation": {
            "provider": "linux_hardened_v1",
            "cgroupRoot": "/sys/fs/cgroup/tempestmiku-m4-disposable",
            "launcher": "/opt/tempestmiku-isolation-runtime/bin/bwrap",
            "runtimeRoots": ["/opt/tempestmiku-isolation-runtime"],
            "linkedRoots": [
                {"path": "/srv/tempestmiku/projects/example", "mode": "rw"}
            ],
            "exclusivity": {
                "status": "not_proven",
                "reason": "No production supervisor lease is represented.",
            },
        },
        "tools": tool_specs(),
        "execution": {
            "timeoutSeconds": 300,
            "environment": {
                "PATH": "/opt/tempestmiku-isolation-runtime/bin",
                "HOME": "/var/lib/tempestmiku/home",
                "TMPDIR": "/var/lib/tempestmiku/tm-m4-acceptance-test",
            },
        },
        "source": {
            "repositoryRoot": "/workspace",
            "expectedRevision": "a" * 40,
            "requireClean": True,
        },
        "workloadSizing": {
            "status": "pending",
            "reason": "Representative workload metrics remain open.",
        },
    }


def host_config() -> dict[str, object]:
    return {
        "linked_folders": [
            {
                "name": "example",
                "path": "/srv/tempestmiku/projects/example",
                "mode": "rw",
                "commands": ["cat"],
                "safe_args": [],
            }
        ],
        "proc_isolation": {
            "provider": "linux_hardened_v1",
            "launcher": "/opt/tempestmiku-isolation-runtime/bin/bwrap",
            "runtime_roots": ["/opt/tempestmiku-isolation-runtime"],
            "limits": {
                "address_space_bytes": 2_147_483_648,
                "process_count": 64,
                "open_files": 256,
            },
            "cgroup_root": "/sys/fs/cgroup/tempestmiku-m4-disposable",
            "cgroup_limits": {
                "memory_max_bytes": 1_073_741_824,
                "memory_swap_max_bytes": 0,
                "pids_max": 64,
                "cpu_quota_micros": 100_000,
                "cpu_period_micros": 100_000,
            },
        },
    }


def identity(
    path: str,
    *,
    kind: str = "directory",
    executable: bool = False,
    content_sha256: str = "2" * 64,
) -> dict[str, object]:
    base: dict[str, object] = {
        "configuredPath": path,
        "canonicalPath": path,
        "device": 10,
        "inode": abs(hash(path)) % 1_000_000 + 1,
        "uid": 0,
        "gid": 0,
        "mode": 0o755 if executable or kind != "file" else 0o644,
    }
    result = {**base, "identitySha256": m4._identity_digest(base)}
    if kind == "file":
        result.update({"contentSha256": content_sha256, "sizeBytes": 42})
    if kind == "tree":
        result.update(
            {"treeSha256": "9" * 64, "entryCount": 3, "totalBytes": 1024}
        )
    return result


def report() -> dict[str, object]:
    cfg = contract()
    environment = {
        **cfg["execution"]["environment"],
        "TM_HOST_CONFIG": cfg["service"]["hostConfigPath"],
        "TM_LINUX_HARDENED_TESTS": "1",
    }
    tool_identities = {}
    for name, spec in cfg["tools"].items():
        tool_identities[name] = identity(
            spec["path"],
            kind="file",
            executable=name != "acceptanceProgram",
            content_sha256=spec["sha256"],
        )
    value: dict[str, object] = {
        "schemaVersion": 1,
        "reportType": m4.REPORT_TYPE,
        "status": "passed",
        "recordedAt": "2026-07-19T00:00:00Z",
        "evidenceClass": m4.EVIDENCE_CLASS,
        "threatModel": cfg["threatModel"],
        "microvm": cfg["microvm"],
        "workloadSizing": cfg["workloadSizing"],
        "contract": {
            "identity": identity(
                "/etc/tempestmiku/m4-contract.json",
                kind="file",
                content_sha256="4" * 64,
            ),
            "schemaVersion": 1,
            "canonicalSha256": m4._canonical_digest(cfg),
        },
        "hostConfig": {
            "identity": identity(
                "/etc/tempestmiku/m4-host.json",
                kind="file",
                content_sha256="5" * 64,
            ),
            "provider": "linux_hardened_v1",
            "rlimits": {
                "addressSpaceBytes": 2_147_483_648,
                "processCount": 64,
                "openFiles": 256,
            },
            "cgroupLimits": {
                "memoryMaxBytes": 1_073_741_824,
                "memorySwapMaxBytes": 0,
                "pidsMax": 64,
                "cpuQuotaMicros": 100_000,
                "cpuPeriodMicros": 100_000,
            },
        },
        "service": {
            "name": "tempestmiku-m4-disposable",
            "role": "all",
            "uid": 23017,
            "gid": 23017,
            "architecture": "x86_64",
            "kernel": "6.8.0",
            "runtimeArtifactIdentity": "tm-server@sha256:" + "2" * 64,
            "environment": cfg["service"]["expectedEnvironment"],
            "semantics": "disposable_fixture_only",
            "serverBinary": identity(
                "/opt/tempestmiku/bin/tm-server",
                kind="file",
                executable=True,
                content_sha256="2" * 64,
            ),
            "serviceManagerConfig": identity(
                "/etc/systemd/system/tempestmiku-m4.service", kind="file"
            ),
            "currentCgroupPath": "/sys/fs/cgroup/tempestmiku-m4-manager",
            "cgroupNamespace": {"device": 4, "inode": 4026531835},
        },
        "tools": tool_identities,
        "isolation": {
            "provider": "linux_hardened_v1",
            "launcher": identity(
                "/opt/tempestmiku-isolation-runtime/bin/bwrap",
                kind="file",
                executable=True,
            ),
            "runtimeRoots": [
                identity("/opt/tempestmiku-isolation-runtime", kind="tree")
            ],
            "linkedRoots": [
                {
                    "mode": "rw",
                    "identity": identity("/srv/tempestmiku/projects/example"),
                }
            ],
            "cgroup": {
                "path": "/sys/fs/cgroup/tempestmiku-m4-disposable",
                "device": 30,
                "inode": 31,
                "uid": 23017,
                "gid": 23017,
                "mountPoint": "/sys/fs/cgroup",
                "controllers": ["cpu", "memory", "pids"],
                "subtreeControl": ["cpu", "memory", "pids"],
                "killWritable": True,
                "emptyBefore": True,
                "emptyAfter": True,
                "unknownChildrenBefore": [],
                "unknownChildrenAfter": [],
                "exclusivity": cfg["isolation"]["exclusivity"],
            },
        },
        "canary": {
            "arguments": m4.CANARY_ARGUMENTS,
            "status": "passed",
            "durationMs": 123,
            "timeoutSeconds": 300,
            "outputBytes": 128,
            "outputSha256": "7" * 64,
            "outputRetained": False,
            "successSummaryMatched": True,
            "environmentKeys": sorted(environment),
            "environmentSha256": m4._canonical_digest(environment),
        },
        "git": {"revision": "a" * 40, "dirty": False, "statusShort": []},
        "sourceSha256": {path: "8" * 64 for path in m4.SOURCE_PATHS},
        "assertions": {key: "passed" for key in m4.REPORT_ASSERTION_KEYS},
        "scopeBoundary": {
            "proven": ["The pinned bounded disposable canary passed."],
            "notProven": [
                "Production service deployment semantics.",
                "Production cgroup exclusivity and supervisor lease.",
                "Production workload sizing.",
                "Hostile host-kernel containment.",
                "microVM isolation.",
            ],
        },
    }
    value["bindingSha256"] = m4._report_binding_digest(value)
    return value


def rebind(value: dict[str, object]) -> None:
    value["bindingSha256"] = m4._report_binding_digest(value)


class ContractValidationTests(unittest.TestCase):
    def test_example_and_fixture_validate(self) -> None:
        example = m4.load_json(
            REPO_ROOT / "tools" / "m4-deployment-contract.example.json"
        )
        self.assertIs(m4.validate_contract(example), example)
        fixture = contract()
        self.assertIs(m4.validate_contract(fixture, live=True), fixture)

    def test_production_service_class_is_unrepresentable(self) -> None:
        value = contract()
        value["evidenceClass"] = "production_service"
        with self.assertRaisesRegex(m4.ValidationError, "refuses production_service"):
            m4.validate_contract(value)

    def test_cgroup_exclusivity_cannot_be_claimed_without_a_lease(self) -> None:
        value = contract()
        value["isolation"]["exclusivity"]["status"] = "proven"
        with self.assertRaisesRegex(m4.ValidationError, "status=not_proven"):
            m4.validate_contract(value)

    def test_execution_environment_is_an_exact_non_secret_allowlist(self) -> None:
        value = contract()
        value["execution"]["environment"]["RUSTC_WRAPPER"] = "/tmp/wrapper"
        with self.assertRaisesRegex(m4.ValidationError, "unknown fields"):
            m4.validate_contract(value)

    def test_tool_must_be_absolute_and_digest_pinned(self) -> None:
        value = contract()
        value["tools"]["canaryExecutable"]["path"] = "target/debug/test"
        with self.assertRaisesRegex(m4.ValidationError, "absolute"):
            m4.validate_contract(value)
        value = contract()
        value["tools"]["git"]["sha256"] = "unbound"
        with self.assertRaisesRegex(m4.ValidationError, "SHA-256"):
            m4.validate_contract(value)

    def test_clean_revision_is_mandatory(self) -> None:
        value = contract()
        value["source"]["requireClean"] = False
        with self.assertRaisesRegex(m4.ValidationError, "requireClean=true"):
            m4.validate_contract(value)

    def test_linked_root_modes_are_contract_bound(self) -> None:
        value = contract()
        config = host_config()
        config["linked_folders"][0]["mode"] = "ro"
        with self.assertRaisesRegex(m4.ValidationError, "linked roots and modes"):
            m4.inspect_host_config(config, value)

    def test_timeout_must_be_bounded(self) -> None:
        value = contract()
        value["execution"]["timeoutSeconds"] = 601
        with self.assertRaisesRegex(m4.ValidationError, "at most 600"):
            m4.validate_contract(value)

    def test_duplicate_json_field_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "duplicate.json"
            path.write_text('{"schemaVersion":1,"schemaVersion":1}')
            with self.assertRaisesRegex(m4.ValidationError, "duplicate field"):
                m4.load_json(path)


class ReportValidationTests(unittest.TestCase):
    def test_valid_report(self) -> None:
        value = report()
        self.assertIs(m4.validate_report(value), value)

    def test_report_binding_detects_scope_tampering(self) -> None:
        value = report()
        value["scopeBoundary"]["notProven"].append("Changed after signing.")
        with self.assertRaisesRegex(m4.ValidationError, "bindingSha256"):
            m4.validate_report(value)

    def test_dirty_source_cannot_be_reported(self) -> None:
        value = report()
        value["git"] = {"revision": "a" * 40, "dirty": True, "statusShort": [" M x"]}
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "clean source revision"):
            m4.validate_report(value)

    def test_runtime_artifact_must_equal_server_binary_content(self) -> None:
        value = report()
        value["service"]["runtimeArtifactIdentity"] = "tm-server@sha256:" + "3" * 64
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "exact server-binary"):
            m4.validate_report(value)

    def test_report_cannot_retain_raw_output(self) -> None:
        value = report()
        value["canary"]["output"] = "possibly sensitive"
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "unknown fields"):
            m4.validate_report(value)
        value = report()
        value["canary"]["outputRetained"] = True
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "must not retain"):
            m4.validate_report(value)

    def test_report_cannot_upgrade_exclusivity_or_production_scope(self) -> None:
        value = report()
        value["isolation"]["cgroup"]["exclusivity"]["status"] = "proven"
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "must not claim cgroup exclusivity"):
            m4.validate_report(value)
        value = report()
        value["scopeBoundary"]["proven"].append("Production service acceptance passed.")
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "must not claim production service"):
            m4.validate_report(value)

    def test_runtime_tree_content_identity_is_required(self) -> None:
        value = report()
        del value["isolation"]["runtimeRoots"][0]["treeSha256"]
        rebind(value)
        with self.assertRaisesRegex(m4.ValidationError, "treeSha256"):
            m4.validate_report(value)

    def test_validate_report_cli_requires_external_expectations(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            m4.parse_args(["validate-report", "/tmp/report.json"])

    def test_external_contract_config_revision_and_artifact_are_all_bound(self) -> None:
        value = report()
        contract_value = contract()
        config_value = host_config()
        with tempfile.TemporaryDirectory() as directory:
            contract_path = Path(directory) / "contract.json"
            config_path = Path(directory) / "host.json"
            contract_path.write_text(json.dumps(contract_value, sort_keys=True))
            config_path.write_text(json.dumps(config_value, sort_keys=True))
            value["contract"]["identity"]["contentSha256"] = m4.sha256_file(contract_path)
            value["hostConfig"]["identity"]["contentSha256"] = m4.sha256_file(config_path)
            rebind(value)
            self.assertIs(
                m4.validate_report_expectations(
                    value,
                    expected_contract_path=contract_path,
                    expected_host_config_path=config_path,
                    expected_revision="a" * 40,
                    expected_runtime_artifact="tm-server@sha256:" + "2" * 64,
                ),
                value,
            )
            with self.assertRaisesRegex(m4.ValidationError, "expected revision"):
                m4.validate_report_expectations(
                    value,
                    expected_contract_path=contract_path,
                    expected_host_config_path=config_path,
                    expected_revision="b" * 40,
                    expected_runtime_artifact="tm-server@sha256:" + "2" * 64,
                )
            config_path.write_text(json.dumps(config_value, indent=2))
            with self.assertRaisesRegex(m4.ValidationError, "expected host config"):
                m4.validate_report_expectations(
                    value,
                    expected_contract_path=contract_path,
                    expected_host_config_path=config_path,
                    expected_revision="a" * 40,
                    expected_runtime_artifact="tm-server@sha256:" + "2" * 64,
                )

    def test_expected_tool_digest_prevents_substitution(self) -> None:
        value = report()
        contract_value = contract()
        config_value = host_config()
        with tempfile.TemporaryDirectory() as directory:
            contract_path = Path(directory) / "contract.json"
            config_path = Path(directory) / "host.json"
            contract_value["tools"]["canaryExecutable"]["sha256"] = "f" * 64
            contract_path.write_text(json.dumps(contract_value, sort_keys=True))
            config_path.write_text(json.dumps(config_value, sort_keys=True))
            value["contract"]["identity"]["contentSha256"] = m4.sha256_file(contract_path)
            value["contract"]["canonicalSha256"] = m4._canonical_digest(contract_value)
            value["hostConfig"]["identity"]["contentSha256"] = m4.sha256_file(config_path)
            rebind(value)
            with self.assertRaisesRegex(m4.ValidationError, "canaryExecutable digest"):
                m4.validate_report_expectations(
                    value,
                    expected_contract_path=contract_path,
                    expected_host_config_path=config_path,
                    expected_revision="a" * 40,
                    expected_runtime_artifact="tm-server@sha256:" + "2" * 64,
                )

    def test_atomic_post_link_validation_failure_removes_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.json"

            def reject(_: dict[str, object]) -> None:
                raise m4.ValidationError("post-link contract mismatch")

            with self.assertRaisesRegex(m4.ValidationError, "post-link contract mismatch"):
                m4._atomic_create_report(
                    output, report(), post_link_validator=reject
                )
            self.assertFalse(output.exists())


class EnvironmentAndSourceTests(unittest.TestCase):
    def test_sanitized_environment_drops_ambient_cargo_and_secrets(self) -> None:
        state = m4._bind_state(
            {
                "stateVersion": 2,
                "contractData": contract(),
                "hostConfig": {
                    "identity": {"configuredPath": "/etc/tempestmiku/m4-host.json"}
                },
            }
        )
        old = os.environ.copy()
        try:
            os.environ["RUSTFLAGS"] = "-C linker=/tmp/evil"
            os.environ["CARGO_ENCODED_RUSTFLAGS"] = "evil"
            os.environ["RUSTC_WRAPPER"] = "/tmp/evil"
            os.environ["AWS_SECRET_ACCESS_KEY"] = "secret"
            environment = m4.sanitized_canary_environment(state)
        finally:
            os.environ.clear()
            os.environ.update(old)
        self.assertEqual(
            set(environment),
            m4.EXECUTION_ENV_KEYS
            | {"TM_HOST_CONFIG", "TM_LINUX_HARDENED_TESTS"},
        )
        self.assertFalse(
            {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "AWS_SECRET_ACCESS_KEY"}
            & set(environment)
        )

    def test_identity_comparison_detects_runtime_tree_mutation(self) -> None:
        before = identity("/opt/runtime", kind="tree")
        after = dict(before)
        after["treeSha256"] = "a" * 64
        with self.assertRaisesRegex(m4.ValidationError, "runtime contents changed"):
            m4._identity_unchanged(before, after, "runtime contents")


class SchemaAlignmentTests(unittest.TestCase):
    def test_schema_required_fields_match_strict_validators(self) -> None:
        contract_schema = m4.load_json(
            REPO_ROOT / "tools" / "m4-deployment-contract.schema.json"
        )
        report_schema = m4.load_json(
            REPO_ROOT / "tools" / "m4-acceptance-report.schema.json"
        )
        self.assertEqual(set(contract_schema["required"]), m4.CONTRACT_TOP_KEYS)
        self.assertEqual(set(report_schema["required"]), m4.REPORT_TOP_KEYS)
        self.assertEqual(
            contract_schema["properties"]["evidenceClass"],
            {"const": m4.EVIDENCE_CLASS},
        )
        self.assertEqual(
            report_schema["properties"]["evidenceClass"],
            {"const": m4.EVIDENCE_CLASS},
        )
        self.assertEqual(
            set(report_schema["properties"]["sourceSha256"]["required"]),
            set(m4.SOURCE_PATHS),
        )
        self.assertEqual(
            set(report_schema["properties"]["assertions"]["required"]),
            m4.REPORT_ASSERTION_KEYS,
        )


if __name__ == "__main__":
    unittest.main()
