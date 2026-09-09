from __future__ import annotations

import importlib.util
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "validation" / "hardening" / "embedded_miri.py"
SPEC = importlib.util.spec_from_file_location("embedded_miri", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
embedded_miri = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = embedded_miri
SPEC.loader.exec_module(embedded_miri)


class EmbeddedMiriTests(unittest.TestCase):
    def test_inventory_names_each_required_component_scenario(self) -> None:
        scenarios = embedded_miri.load_inventory()

        self.assertEqual(
            [(scenario.component, scenario.identifier) for scenario in scenarios],
            [
                ("sx126x", "sx126x-state-machine"),
                ("lr1110", "lr1110-state-machine"),
                ("embedded-persistence", "flash-journal-state-machine"),
            ],
        )
        self.assertEqual(len(scenarios[2].quick_filters), 3)

    def test_quick_and_full_modes_select_explicit_borrow_models(self) -> None:
        self.assertEqual(
            embedded_miri.models(embedded_miri.Mode.QUICK),
            (embedded_miri.BorrowModel.STACKED,),
        )
        self.assertEqual(
            embedded_miri.models(embedded_miri.Mode.FULL),
            (
                embedded_miri.BorrowModel.STACKED,
                embedded_miri.BorrowModel.TREE,
            ),
        )

    def test_miri_test_count_comes_from_successful_runner_output(self) -> None:
        output = (
            b"running 16 tests\n"
            b"test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; "
            b"67 filtered out\n"
        )
        self.assertEqual(embedded_miri.parse_completed_tests(output), 16)
        with self.assertRaises(embedded_miri.EmbeddedMiriError):
            embedded_miri.parse_completed_tests(b"test result: FAILED")

    def test_runner_clears_only_owned_component_evidence(self) -> None:
        scenarios = embedded_miri.load_inventory()
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            owned = artifacts / "sx126x.assurance.json"
            log = artifacts / "sx126x-stacked.log"
            unrelated = artifacts / "result.json"
            for path in (owned, log, unrelated):
                path.write_text("evidence", encoding="utf-8")

            embedded_miri.clear_owned_artifacts(scenarios, artifacts)

            self.assertFalse(owned.exists())
            self.assertFalse(log.exists())
            self.assertTrue(unrelated.exists())

    def test_artifact_directory_is_validated_before_creation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            root = temporary / "artifacts"
            outside = temporary / "outside"
            with mock.patch.dict(
                os.environ,
                {
                    "PRNS_VALIDATION_ARTIFACT_ROOT": str(root),
                    "PRNS_VALIDATION_ARTIFACT_DIR": str(outside),
                },
            ):
                with self.assertRaises(embedded_miri.EmbeddedMiriError):
                    embedded_miri.artifact_directory()
            self.assertFalse(outside.exists())

    def test_duplicate_component_inventory_is_rejected(self) -> None:
        scenario = """
schema = 1
[[scenario]]
component = "sx126x"
id = "sx126x-state-machine"
manifest = "prns-interfaces/impls/embassy/Cargo.toml"
features = ["lora"]
quick_filters = ["one"]
full_filters = ["one"]
sources = ["prns-interfaces/impls/embassy/src/radios/sx126x.rs"]
[[scenario]]
component = "sx126x"
id = "another-scenario"
manifest = "prns-interfaces/impls/embassy/Cargo.toml"
features = ["lora"]
quick_filters = ["two"]
full_filters = ["two"]
sources = ["prns-interfaces/impls/embassy/src/radios/sx126x.rs"]
"""
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "inventory.toml"
            inventory.write_text(scenario, encoding="utf-8")
            with self.assertRaises(embedded_miri.EmbeddedMiriError):
                embedded_miri.load_inventory(inventory)


if __name__ == "__main__":
    unittest.main()
