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

    def test_generated_contract_change_checks_both_language_consumers(self) -> None:
        names = self.gate_names({"prns-host/schema/host-contract-v1.json"})

        self.assertIn("JavaScript clean generated output", names)
        self.assertIn("JavaScript and TypeScript contract check", names)
        self.assertIn("JVM binding compile", names)

    def test_lockfile_change_checks_only_its_workspace_for_advisories(self) -> None:
        names = self.gate_names({"prnsd/Cargo.lock"})

        self.assertIn("RustSec advisories (prnsd/Cargo.lock)", names)
        self.assertNotIn("RustSec advisories (Cargo.lock)", names)

    def test_documentation_change_has_no_additional_ci_lane(self) -> None:
        self.assertEqual(self.gate_names({"docs/architecture.md"}), set())


if __name__ == "__main__":
    unittest.main()
