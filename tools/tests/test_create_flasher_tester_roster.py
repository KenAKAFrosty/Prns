from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
RELEASE_TOOLS = ROOT / "tools" / "release"
if str(RELEASE_TOOLS) not in sys.path:
    sys.path.insert(0, str(RELEASE_TOOLS))
from flasher_board_catalog import release_boards

SCRIPT = RELEASE_TOOLS / "create-flasher-tester-roster.py"
SPEC = importlib.util.spec_from_file_location("create_flasher_tester_roster", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
CREATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CREATOR)


class TesterRosterCreatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.catalog = self.root / "boards.json"
        self.document = json.loads(
            (ROOT / "release" / "flash" / "boards.json").read_text(encoding="utf-8")
        )
        self.catalog.write_text(json.dumps(self.document), encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def create(self, name: str) -> dict:
        output = self.root / name
        CREATOR.create(
            ROOT / "release" / "acceptance" / "roster-template.json",
            self.catalog,
            "0.4.0",
            output,
        )
        return json.loads(output.read_text(encoding="utf-8"))

    def test_current_roster_contains_exactly_shipping_boards(self) -> None:
        roster = self.create("current.json")
        expected = {
            board["slug"]
            for board in self.document["boards"]
            if board["availability"] == "shipping"
        }
        self.assertEqual(roster["schema"], 3)
        self.assertEqual(roster["release"], {"version": "0.4.0"})
        self.assertEqual(
            {assignment["board"] for assignment in roster["physical_assignments"]},
            expected,
        )
        self.assertEqual(len(roster["physical_assignments"]), len(expected) * 2)

    def test_promoted_board_enters_roster_without_template_changes(self) -> None:
        for board in self.document["boards"]:
            if board["slug"] == "heltec-wireless-stick-lite-v3":
                board["availability"] = "shipping"
        self.catalog.write_text(json.dumps(self.document), encoding="utf-8")
        roster = self.create("promoted.json")
        assignments = {
            (assignment["board"], assignment["surface"])
            for assignment in roster["physical_assignments"]
        }
        self.assertTrue(
            {
                ("heltec-wireless-stick-lite-v3", "cli"),
                ("heltec-wireless-stick-lite-v3", "web"),
            }
            <= assignments
        )

    def test_refuses_to_overwrite_a_roster(self) -> None:
        self.create("existing.json")
        with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
            self.create("existing.json")

    def test_candidate_qualification_tools_use_the_manifest_snapshot(self) -> None:
        candidate = self.root / "candidate"
        qualification = candidate / "qualification"
        qualification.mkdir(parents=True)
        manifest = {
            "schema": 3,
            "targets": [
                {"board_slug": "first", "transport": "esp-serial"},
                {"board_slug": "second", "transport": "uf2-mass-storage"},
            ],
        }
        (candidate / "flash-manifest.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )
        boards = release_boards(qualification / "flasher_acceptance_contract.py")
        self.assertEqual(boards.shipping, ("first", "second"))
        self.assertEqual(boards.esp_serial, ("first",))


if __name__ == "__main__":
    unittest.main()
