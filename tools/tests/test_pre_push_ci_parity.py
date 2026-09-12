from __future__ import annotations

import importlib.util
import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "validation" / "hygiene" / "pre-push-ci-parity.py"
sys.path.insert(0, str(SCRIPT_PATH.parent))
SPEC = importlib.util.spec_from_file_location("pre_push_ci_parity", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
parity = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = parity
SPEC.loader.exec_module(parity)


class PrePushCiParityTests(unittest.TestCase):
    def gate_names(self, paths: set[str]) -> set[str]:
        return {gate.name for gate in parity.plan_for_paths(paths).gates}

    def test_core_change_checks_root_feature_and_native_consumers(self) -> None:
        names = self.gate_names({"prns-core/src/engine.rs"})

        self.assertIn("root Clippy", names)
        self.assertIn("prns-core external-allocation lane", names)
        self.assertIn("Node native binding Clippy", names)
        self.assertIn("Tokio runtime all-features Clippy", names)
        self.assertIn("Tokio umbrella feature-family Clippy", names)
        self.assertIn("validation integration capstones Clippy", names)
        self.assertIn("prnsd all-features Clippy", names)
        self.assertIn("prns-wasm wasm32 Clippy", names)
        self.assertIn("JavaScript browser package smoke", names)
        self.assertIn("embedded resource matrix", names)
        self.assertIn("Embassy runtime Clippy", names)
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
        ).gates
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
        gates = parity.plan_for_paths({"prnsd/Cargo.lock"}).gates
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

    def test_embassy_change_links_the_exact_resource_matrix(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/impls/embassy/src/runtime/request_runner.rs"}
        ).gates
        names = {gate.name for gate in gates}

        self.assertIn("embedded resource matrix", names)
        gate = next(gate for gate in gates if gate.name == "Embassy runtime Clippy")
        self.assertEqual(gate.cwd, ROOT / "prns-runtime/impls/embassy")
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(
            gate.command,
            (
                "cargo",
                "clippy",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_memory_catalog_and_toolchain_changes_link_the_resource_matrix(self) -> None:
        for path in (
            "personal-hopspot/memory/src/profiles/nrf52840.rs",
            "prns-flash-manifest/src/catalog.rs",
            "personal-hopspot/builder/src/toolchain.rs",
            "tools/release/release-esp-toolchain-identity.sh",
        ):
            with self.subTest(path=path):
                gates = parity.plan_for_paths({path}).gates
                gate = next(
                    gate for gate in gates if gate.name == "embedded resource matrix"
                )
                self.assertEqual(
                    gate.command,
                    (
                        "./tools/prns",
                        "build",
                        "embedded",
                        "resources",
                        "report",
                        "--all",
                    ),
                )

    def test_shared_component_runs_miri_and_every_target_isa_suite(self) -> None:
        plan = parity.plan_for_paths(
            {"prns-interfaces/impls/embassy/src/radios/sx126x.rs"}
        )
        miri = next(gate for gate in plan.gates if gate.name == "embedded Miri")
        isa = next(
            gate for gate in plan.gates if gate.name == "embedded target-ISA"
        )
        self.assertEqual(
            miri.command,
            (
                "python3",
                "validation/run.py",
                "run",
                "--suite",
                "embedded-miri-quick",
            ),
        )
        self.assertEqual(
            miri.env,
            (("PRNS_EMBEDDED_MIRI_PROVISIONING", "require-existing"),),
        )
        self.assertEqual(
            isa.command,
            (
                "python3",
                "validation/run.py",
                "run",
                "--suite",
                "embedded-isa-thumbv7em",
                "--suite",
                "embedded-isa-riscv32imac",
                "--suite",
                "embedded-isa-xtensa-esp32s3",
            ),
        )

    def test_board_change_runs_only_its_selected_target_isa_suite(self) -> None:
        plan = parity.plan_for_paths(
            {"personal-hopspot/embedded/nrf52840/src/bin/t096.rs"}
        )
        isa = next(
            gate for gate in plan.gates if gate.name == "embedded target-ISA"
        )
        self.assertEqual(
            isa.command,
            (
                "python3",
                "validation/run.py",
                "run",
                "--suite",
                "embedded-isa-thumbv7em",
            ),
        )
        self.assertFalse(any(gate.name == "embedded Miri" for gate in plan.gates))

    def test_pre_push_plan_preserves_the_classifier_result_and_defers_pilots(self) -> None:
        paths = {"personal-hopspot/memory/src/profiles/nrf52840/mod.rs"}
        plan = parity.plan_for_paths(paths)
        self.assertEqual(plan.assurance, parity.selection_for_paths(paths))
        self.assertEqual(
            plan.assurance.suite_ids(parity.Lane.PILOTS),
            ("embedded-platform-nrf52840",),
        )
        self.assertFalse(any("platform" in gate.name for gate in plan.gates))

        output = io.StringIO()
        with mock.patch.object(
            parity, "changed_paths", return_value=paths
        ), mock.patch.object(
            sys,
            "argv",
            [
                "pre-push-ci-parity.py",
                "--update",
                "a" * 40,
                "b" * 40,
                "--plan",
            ],
        ), redirect_stdout(output):
            self.assertEqual(parity.main(), 0)
        rendered = output.getvalue()
        self.assertIn("selected resources suites", rendered)
        self.assertIn("embedded-builds", rendered)
        self.assertIn("selected isa suites", rendered)
        self.assertIn("embedded-isa-thumbv7em", rendered)
        self.assertIn("deferred scheduled/release pilots", rendered)
        self.assertIn("embedded-platform-nrf52840", rendered)
        self.assertEqual(
            rendered.count(
                "matched personal-hopspot/memory/src/profiles/nrf52840/mod.rs"
            ),
            4,
        )

    def test_pilot_only_change_is_reported_even_without_an_executed_gate(self) -> None:
        paths = {
            "validation/hardening/embedded_platform/platform/nrf52840.py"
        }
        plan = parity.plan_for_paths(paths)
        self.assertEqual(plan.gates, ())

        output = io.StringIO()
        with mock.patch.object(
            parity, "changed_paths", return_value=paths
        ), mock.patch.object(
            sys,
            "argv",
            [
                "pre-push-ci-parity.py",
                "--update",
                "a" * 40,
                "b" * 40,
                "--plan",
            ],
        ), redirect_stdout(output):
            self.assertEqual(parity.main(), 0)

        rendered = output.getvalue()
        self.assertIn("deferred scheduled/release pilots", rendered)
        self.assertIn("embedded-platform-nrf52840", rendered)
        self.assertNotIn("no additional CI lanes selected", rendered)

    def test_tokio_runtime_change_runs_all_features_clippy(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/impls/tokio/src/manifold/driver/mod.rs"}
        ).gates
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
        ).gates
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

    def test_runtime_change_runs_validation_integration_capstones_clippy(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/impls/tokio/src/manifold/driver/mod.rs"}
        ).gates
        gate = next(
            gate
            for gate in gates
            if gate.name == "validation integration capstones Clippy"
        )

        self.assertEqual(gate.cwd, ROOT / "validation/integration")
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(
            gate.command,
            (
                "cargo",
                "clippy",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_runtime_change_runs_prnsd_all_features_clippy(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/core/src/runtime/observability.rs"}
        ).gates
        gate = next(
            gate for gate in gates if gate.name == "prnsd all-features Clippy"
        )

        self.assertEqual(gate.cwd, ROOT / "prnsd")
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(
            gate.command,
            (
                "cargo",
                "clippy",
                "--workspace",
                "--all-features",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_core_change_runs_prns_wasm_wasm32_clippy(self) -> None:
        gates = parity.plan_for_paths({"prns-core/src/engine.rs"}).gates
        gate = next(gate for gate in gates if gate.name == "prns-wasm wasm32 Clippy")

        self.assertEqual(gate.cwd, ROOT / "prns-wasm")
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(
            gate.command,
            (
                "cargo",
                "clippy",
                "--target",
                "wasm32-unknown-unknown",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_runtime_change_runs_javascript_browser_package_smoke(self) -> None:
        gates = parity.plan_for_paths(
            {"prns-runtime/core/src/runtime/observability.rs"}
        ).gates
        gate = next(
            gate
            for gate in gates
            if gate.name == "JavaScript browser package smoke"
        )

        self.assertEqual(gate.cwd, ROOT / "prns-js")
        self.assertEqual(gate.env, (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),))
        self.assertEqual(gate.command, ("npm", "run", "test:browser:full"))

    def test_documentation_change_has_no_additional_ci_lane(self) -> None:
        self.assertEqual(self.gate_names({"docs/architecture.md"}), set())


if __name__ == "__main__":
    unittest.main()
