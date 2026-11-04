"""Selected optional backends must be represented in both release inventories."""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def module(name: str, path: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    assert spec is not None and spec.loader is not None
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


class InventoryFeatureTests(unittest.TestCase):
    def test_notice_generation_selects_only_explicit_features(self):
        notices = module("selected_notices", "tools/repo/generate-third-party-notices.py")

        def complete(command, **_kwargs):
            output = Path(command[command.index("--output-file") + 1])
            output.write_text(json.dumps({"licenses": []}))
            return subprocess.CompletedProcess(command, 0, "", "")

        for features in [(), ("redb",)]:
            with self.subTest(features=features), tempfile.TemporaryDirectory() as directory:
                with mock.patch.object(notices.subprocess, "run", side_effect=complete) as run:
                    notices.generate_graph("prns-storage/Cargo.toml", "x86_64-unknown-linux-gnu", Path(directory), Path(directory), "cargo-about", features)
                command = run.call_args.args[0]
                self.assertEqual("--features" in command, bool(features))
                if features:
                    self.assertEqual(command[command.index("--features") + 1], "redb")
                self.assertNotIn("--all-features", command)
                self.assertIn("--locked", command)

    def test_unsafe_metadata_selects_only_explicit_features(self):
        audit = module("selected_unsafe", "validation/security/unsafe-audit.py")
        for features in [(), ("redb",)]:
            with self.subTest(features=features):
                result = subprocess.CompletedProcess([], 0, "{}", "")
                with mock.patch.object(audit.subprocess, "run", return_value=result) as run:
                    self.assertEqual(audit.run_metadata("prns-storage/Cargo.toml", "x86_64-unknown-linux-gnu", features), {})
                command = run.call_args.args[0]
                self.assertEqual("--features" in command, bool(features))
                if features:
                    self.assertEqual(command[command.index("--features") + 1], "redb")
                self.assertNotIn("--all-features", command)
                self.assertIn("--locked", command)
