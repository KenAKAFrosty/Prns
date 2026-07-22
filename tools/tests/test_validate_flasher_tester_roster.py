from __future__ import annotations

from copy import deepcopy
from datetime import date
import importlib.util
from pathlib import Path
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "release" / "validate-flasher-tester-roster.py"
SPEC = importlib.util.spec_from_file_location("validate_flasher_tester_roster", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)

VERSION = "0.2.6"


def complete_roster() -> dict:
    assignments = []
    for index, (target, (os_name, architecture)) in enumerate(VALIDATOR.CLI_TARGETS.items()):
        assignments.append(
            {
                "os": os_name,
                "architecture": architecture,
                "cli_target": target,
                "web_browser": {
                    "name": "edge" if os_name == "windows" else "chrome",
                    "channel": "stable",
                },
                "tester": f"github:tester-{index}",
                "boards": list(VALIDATOR.SHIPPING_BOARDS),
                "cables_ready": True,
                "device_permissions_ready": True,
                "recovery_instructions_reviewed": True,
            }
        )
    return {
        "schema": 1,
        "release": {"version": VERSION},
        "release_owner": "github:release-owner",
        "confirmed_on": date.today().isoformat(),
        "assignments": assignments,
    }


class TesterRosterValidatorTests(unittest.TestCase):
    def validate(self, roster: dict) -> list[str]:
        return VALIDATOR.validate(roster, VERSION)

    def test_complete_five_architecture_roster_passes(self) -> None:
        self.assertEqual(self.validate(complete_roster()), [])

    def test_missing_architecture_and_duplicate_assignment_fail(self) -> None:
        roster = complete_roster()
        roster["assignments"][-1] = deepcopy(roster["assignments"][0])
        errors = self.validate(roster)
        self.assertTrue(any("duplicate tester assignment" in error for error in errors))
        self.assertTrue(any("missing host architectures" in error for error in errors))

    def test_placeholder_or_email_identity_fails(self) -> None:
        roster = complete_roster()
        roster["release_owner"] = "TODO"
        roster["assignments"][0]["tester"] = "private@example.com"
        errors = self.validate(roster)
        self.assertTrue(any("release_owner" in error for error in errors))
        self.assertTrue(any("tester identity" in error for error in errors))

    def test_wrong_browser_target_and_readiness_fail(self) -> None:
        roster = complete_roster()
        assignment = roster["assignments"][0]
        assignment["web_browser"] = {"name": "firefox", "channel": "stable"}
        assignment["cli_target"] = "x86_64-pc-windows-msvc"
        assignment["cables_ready"] = False
        errors = self.validate(roster)
        self.assertTrue(any("web_browser must be stable" in error for error in errors))
        self.assertTrue(any("cli_target does not match" in error for error in errors))
        self.assertTrue(any("cables_ready" in error for error in errors))

    def test_all_four_unique_shipping_boards_are_required(self) -> None:
        roster = complete_roster()
        roster["assignments"][0]["boards"][-1] = roster["assignments"][0]["boards"][0]
        self.assertTrue(any("all four shipping boards" in error for error in self.validate(roster)))

    def test_malformed_board_values_fail_without_crashing(self) -> None:
        roster = complete_roster()
        roster["assignments"][0]["boards"] = [{"private": "identifier"}]
        self.assertTrue(any("all four shipping boards" in error for error in self.validate(roster)))

    def test_roster_is_bound_to_candidate_identity(self) -> None:
        roster = complete_roster()
        roster["release"]["version"] = "0.2.7"
        self.assertTrue(any("differs from the candidate" in error for error in self.validate(roster)))


if __name__ == "__main__":
    unittest.main()
