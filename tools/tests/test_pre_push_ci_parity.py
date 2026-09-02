from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "validation" / "hygiene" / "pre-push-ci-parity.py"
SPEC = importlib.util.spec_from_file_location("pre_push_ci_parity", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
parity = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = parity
SPEC.loader.exec_module(parity)


class PrePushCiParityTests(unittest.TestCase):
    def gate_names(self, paths: set[str]) -> set[str]:
        return {gate.name for gate in parity.plan_for_paths(paths)}

    def test_core_change_checks_root_feature_and_native_consumers(self) -> None:
        names = self.gate_names({"prns-core/src/engine.rs"})

        self.assertIn("root Clippy", names)
        self.assertIn("prns-core external-allocation lane", names)
        self.assertIn("Node native binding Clippy", names)
        self.assertIn("Tokio runtime all-features Clippy", names)
        self.assertIn("Tokio umbrella feature-family Clippy", names)
        self.assertIn("embedded build matrix", names)
        self.assertIn("unsafe dependency inventory", names)

    def test_generated_contract_change_checks_both_language_consumers(self) -> None:
        names = self.gate_names({"prns-host/schema/host-contract-v1.json"})

        self.assertIn("JavaScript clean generated output", names)
        self.assertIn("JavaScript and TypeScript contract check", names)
        self.assertIn("JVM binding compile", names)
        self.assertIn("Swift host contract smoke", names)

    def test_swift_binding_change_runs_contract_smoke(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-host/bindings/swift/Sources/PersonalRns/Command.swift"}
        )
        gate = next(
            gate for gate in gates if gate.name == "Swift host contract smoke"
        )

        self.assertEqual(gate.cwd, ROOT)
        self.assertEqual(
            gate.command,
            (
                "python3",
                "-m",
                "validation.interop.cases.host_swift_contract_smoke",
            ),
        )

    def test_lockfile_change_runs_scoped_policy_and_unsafe_checks(self) -> None:
        gates = parity.plan_for_paths({"prnsd/Cargo.lock"})
        names = {gate.name for gate in gates}

        self.assertIn("license policy parity", names)
        self.assertIn("dependency policy (prnsd/Cargo.lock)", names)
        self.assertIn("unsafe dependency inventory", names)
        policy = next(
            gate
            for gate in gates
            if gate.name == "dependency policy (prnsd/Cargo.lock)"
        )
        self.assertEqual(
            policy.command[-4:],
            ("advisories", "licenses", "sources", "bans"),
        )

    def test_embassy_change_runs_the_exact_embedded_matrix(self) -> None:
        names = self.gate_names(
            {"prns-runtime/impls/embassy/src/runtime/request_runner.rs"}
        )

        self.assertIn("embedded build matrix", names)

    def test_tokio_runtime_change_runs_all_features_clippy(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/impls/tokio/src/manifold/driver/mod.rs"}
        )
        gate = next(
            gate for gate in gates if gate.name == "Tokio runtime all-features Clippy"
        )

        self.assertEqual(gate.cwd, ROOT / "prns-runtime/impls/tokio")
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(
            gate.command,
            (
                "cargo",
                "clippy",
                "--all-features",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_tokio_runtime_change_runs_umbrella_feature_family_clippy(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/impls/tokio/src/manifold/driver/mod.rs"}
        )
        gate = next(
            gate
            for gate in gates
            if gate.name == "Tokio umbrella feature-family Clippy"
        )

        self.assertEqual(gate.cwd, ROOT)
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(
            gate.command,
            (
                "cargo",
                "clippy",
                "-p",
                "personal-rns",
                "--features",
                "tokio-host,tcp,udp,wifi-auto,shared-instance",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_documentation_change_has_no_additional_ci_lane(self) -> None:
        self.assertEqual(self.gate_names({"docs/architecture.md"}), set())


if __name__ == "__main__":
    unittest.main()
