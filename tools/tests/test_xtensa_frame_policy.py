"""Shipping Xtensa selection and strict ELF alignment evidence remain coupled."""
from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("xtensa_frame_check", ROOT / "validation/platforms/xtensa-frame-check.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class XtensaFramePolicyTests(unittest.TestCase):
    def prepare(self, root, targets=("fixture-a", "fixture-b")):
        baseline = root / "personal-hopspot/resources/baseline/canonical.json"
        baseline.parent.mkdir(parents=True)
        baseline.write_text(json.dumps({"targets": [
            {"target": {"id": name}, "architecture": {"rust_target": GATE.TARGET}}
            for name in targets] + [{"target": {"id": "riscv"}, "architecture": {"rust_target": "riscv32imac-unknown-none-elf"}}]}))
        artifacts = root / "target/flash-artifacts/resources/configured"
        (artifacts / "reports").mkdir(parents=True)
        for name in targets:
            report = {"target": {"id": name}, "status": {"kind": "success"},
                      "architecture": {"rust_target": GATE.TARGET, "rustflags": GATE.POLICY},
                      "build": {"binary": name}}
            (artifacts / "reports" / (name + ".json")).write_text(json.dumps(report))
            elf = artifacts / "cargo-cache" / GATE.TARGET / "release" / name
            elf.parent.mkdir(parents=True, exist_ok=True)
            elf.write_bytes(name.encode())
        return artifacts

    def audit(self, command, *, check, bad=0, functions=12, instructions=30, unknown=4):
        self.assertTrue(check)
        elf, output = map(Path, command[2:4])
        output.write_text(json.dumps({"sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
                                     "functions": functions, "instruction_count": instructions, "movsp_count": bad + unknown,
                                     "misaligned_immediate_count": bad, "unknown_adjustment_count": unknown}))

    def test_all_owned_xtensa_targets_are_audited_and_unknowns_remain(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); artifacts = self.prepare(root)
            with mock.patch.object(GATE.subprocess, "run", side_effect=self.audit) as run:
                rows = GATE.check(root)
            self.assertEqual(run.call_count, 2)
            self.assertEqual([row["target"] for row in rows], ["fixture-a", "fixture-b"])
            self.assertEqual([row["unknown_adjustment_count"] for row in rows], [4, 4])
            self.assertEqual(json.loads((artifacts / "xtensa-frame-summary.json").read_text())["failed_targets"], [])

    def test_known_misalignment_fails_and_preserves_all_reports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); artifacts = self.prepare(root)
            with mock.patch.object(GATE.subprocess, "run", side_effect=lambda command, check: self.audit(command, check=check, bad=1)):
                with self.assertRaisesRegex(RuntimeError, "misaligned immediate"):
                    GATE.check(root)
            self.assertEqual(len(json.loads((artifacts / "xtensa-frame-summary.json").read_text())["profiles"]), 2)

    def test_empty_symbol_audit_fails_even_when_no_bad_frames_were_found(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); self.prepare(root)
            with mock.patch.object(GATE.subprocess, "run", side_effect=lambda command, check: self.audit(command, check=check, functions=0, unknown=0)):
                with self.assertRaisesRegex(RuntimeError, "no function symbols"):
                    GATE.check(root)

    def test_symbols_without_parsed_instructions_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); self.prepare(root)
            with mock.patch.object(GATE.subprocess, "run", side_effect=lambda command, check: self.audit(command, check=check, instructions=0, unknown=0)):
                with self.assertRaisesRegex(RuntimeError, "no instructions"):
                    GATE.check(root)

    def test_zero_movsp_is_legitimate_when_functions_were_inspected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); self.prepare(root)
            with mock.patch.object(GATE.subprocess, "run", side_effect=lambda command, check: self.audit(command, check=check, unknown=0)):
                self.assertEqual(len(GATE.check(root)), 2)

    def test_negative_and_malformed_counts_fail(self):
        valid = {"functions": 12, "instruction_count": 30, "movsp_count": 4, "misaligned_immediate_count": 0, "unknown_adjustment_count": 4}
        for name in valid:
            for value in (-1, "0", None, True):
                with self.subTest(name=name, value=value):
                    with self.assertRaisesRegex(RuntimeError, "malformed"):
                        GATE.validate_counts({**valid, name: value}, "fixture")
        with self.assertRaisesRegex(RuntimeError, "inconsistent"):
            GATE.validate_counts({**valid, "movsp_count": 3}, "fixture")

    def test_missing_selected_report_cannot_silently_reduce_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); artifacts = self.prepare(root)
            (artifacts / "reports/fixture-a.json").unlink()
            with self.assertRaises(FileNotFoundError):
                GATE.check(root)

    def test_old_policy_report_is_rejected_before_audit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); artifacts = self.prepare(root)
            path = artifacts / "reports/fixture-a.json"
            value = json.loads(path.read_text()); value["architecture"]["rustflags"][-1] = "force-frame-pointers"
            path.write_text(json.dumps(value))
            with mock.patch.object(GATE.subprocess, "run") as run:
                with self.assertRaisesRegex(RuntimeError, "current-policy"):
                    GATE.check(root)
                run.assert_not_called()

    def test_helper_changes_select_the_owning_shipping_gate(self):
        spec = importlib.util.spec_from_file_location("xtensa_selection", ROOT / "validation/hygiene/embedded_assurance_selection.py")
        selection = importlib.util.module_from_spec(spec)
        import sys
        sys.modules[spec.name] = selection
        spec.loader.exec_module(selection)
        for path in (
            "validation/platforms/xtensa-frame-check.py",
            "validation/platforms/xtensa-frame-audit.py",
            "tools/tests/test_xtensa_frame_policy.py",
            "validation/platforms/fixtures/xtensa-call-frame/src/lib.rs",
        ):
            self.assertTrue(selection.selection_for_paths([path]).includes(selection.Suite.ESP32_FIRMWARE))

    def test_shipping_gate_retains_audit_reports(self):
        self.assertIn("python3 validation/platforms/xtensa-frame-check.py", (ROOT / "validation/platforms/esp32-firmware-check.sh").read_text())
        for workflow in ("ci.yml", "release-readiness.yml"):
            source = (ROOT / ".github/workflows" / workflow).read_text()
            self.assertIn("target/flash-artifacts/resources/configured/work/*/frame-alignment.json", source)
            self.assertIn("target/flash-artifacts/resources/configured/xtensa-frame-summary.json", source)


if __name__ == "__main__":
    unittest.main()
