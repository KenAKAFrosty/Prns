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
    hosts = {
        ("heltec-v4", "cli"): ("linux", "x86_64"),
        ("heltec-v4", "web"): ("linux", "x86_64"),
        ("heltec-v4-r8", "cli"): ("linux", "x86_64"),
        ("heltec-v4-r8", "web"): ("linux", "x86_64"),
        ("t-beam-supreme", "cli"): ("macos", "aarch64"),
        ("t-beam-supreme", "web"): ("macos", "aarch64"),
        ("xiao-esp32-c6", "cli"): ("windows", "x86_64"),
        ("xiao-esp32-c6", "web"): ("windows", "x86_64"),
        ("t-echo", "cli"): ("linux", "aarch64"),
        ("t-echo", "web"): ("macos", "x86_64"),
        ("t114", "cli"): ("linux", "x86_64"),
        ("t114", "web"): ("macos", "aarch64"),
        ("t096", "cli"): ("linux", "aarch64"),
        ("t096", "web"): ("macos", "aarch64"),
        ("t1000-e", "cli"): ("macos", "x86_64"),
        ("t1000-e", "web"): ("windows", "x86_64"),
    }
    physical_assignments = []
    for (board, surface), (os_name, architecture) in hosts.items():
        assignment = {
            "board": board,
            "surface": surface,
            "os": os_name,
            "architecture": architecture,
            "tester": "github:solo-tester",
            "cables_ready": True,
            "device_permissions_ready": True,
            "recovery_instructions_reviewed": True,
        }
        if surface == "web":
            assignment["browser"] = {
                "name": "edge" if os_name == "windows" else "chrome",
                "channel": "stable",
            }
        physical_assignments.append(assignment)
    web_serial_assignments = [
        {
            "board": board,
            "os": os_name,
            "architecture": architecture,
            "browser": {"name": "firefox", "channel": "stable"},
            "tester": "github:solo-tester",
            "cables_ready": True,
            "device_permissions_ready": True,
            "recovery_instructions_reviewed": True,
        }
        for board, os_name, architecture in (
            ("heltec-v4", "linux", "x86_64"),
            ("t-beam-supreme", "macos", "x86_64"),
            ("xiao-esp32-c6", "windows", "x86_64"),
        )
    ]
    fallback_assignments = [
        {
            "browser": {"name": browser, "channel": "stable"},
            "os": os_name,
            "architecture": architecture,
            "tester": "github:solo-tester",
            "browser_ready": True,
        }
        for browser, os_name, architecture in (("safari", "macos", "aarch64"),)
    ]
    installation_assignments = [
        {
            "target": target,
            "os": os_name,
            "architecture": architecture,
            "tester": "github:solo-tester",
            "archive_ready": True,
        }
        for target, (os_name, architecture) in VALIDATOR.CLI_TARGETS.items()
    ]
    return {
        "schema": 3,
        "release": {"version": VERSION},
        "release_owner": "github:release-owner",
        "confirmed_on": date.today().isoformat(),
        "physical_assignments": physical_assignments,
        "web_serial_assignments": web_serial_assignments,
        "fallback_assignments": fallback_assignments,
        "installation_assignments": installation_assignments,
    }


class TesterRosterValidatorTests(unittest.TestCase):
    def validate(self, roster: dict) -> list[str]:
        return VALIDATOR.validate(roster, VERSION)

    def test_one_tester_can_hold_every_assignment(self) -> None:
        self.assertEqual(self.validate(complete_roster()), [])

    def test_missing_and_duplicate_physical_assignment_fail(self) -> None:
        roster = complete_roster()
        roster["physical_assignments"][-1] = deepcopy(
            roster["physical_assignments"][0]
        )
        errors = self.validate(roster)
        self.assertTrue(any("duplicate physical assignment" in error for error in errors))
        self.assertTrue(any("missing physical assignments" in error for error in errors))

    def test_placeholder_or_email_identity_fails(self) -> None:
        roster = complete_roster()
        roster["release_owner"] = "TODO"
        roster["physical_assignments"][0]["tester"] = "private@example.com"
        errors = self.validate(roster)
        self.assertTrue(any("release_owner" in error for error in errors))
        self.assertTrue(any("tester identity" in error for error in errors))

    def test_wrong_browser_host_and_readiness_fail(self) -> None:
        roster = complete_roster()
        assignment = roster["physical_assignments"][1]
        assignment["browser"] = {"name": "firefox", "channel": "stable"}
        assignment["os"] = "windows"
        assignment["architecture"] = "aarch64"
        assignment["cables_ready"] = False
        errors = self.validate(roster)
        self.assertTrue(any("browser must be stable" in error for error in errors))
        self.assertTrue(any("supported host architecture" in error for error in errors))
        self.assertTrue(any("cables_ready" in error for error in errors))

    def test_each_surface_must_cover_linux_macos_and_windows(self) -> None:
        roster = complete_roster()
        for assignment in roster["physical_assignments"]:
            if assignment["surface"] == "cli" and assignment["os"] == "macos":
                assignment["os"] = "linux"
        errors = self.validate(roster)
        self.assertTrue(
            any("cli physical assignments do not cover host OSes" in error for error in errors)
        )

    def test_fallback_and_installer_assignments_are_exact(self) -> None:
        roster = complete_roster()
        roster["fallback_assignments"].pop()
        roster["installation_assignments"][0]["target"] = "x86_64-pc-windows-msvc"
        errors = self.validate(roster)
        self.assertTrue(any("missing fallback assignments" in error for error in errors))
        self.assertTrue(any("duplicate installation assignment" in error for error in errors))

    def test_firefox_web_serial_assignments_require_exact_hosts_and_esp_boards(self) -> None:
        roster = complete_roster()
        roster["web_serial_assignments"][0]["board"] = "t-echo"
        roster["web_serial_assignments"][2]["architecture"] = "aarch64"
        errors = self.validate(roster)
        self.assertTrue(any("eligible shipping ESP-serial board" in error for error in errors))
        self.assertTrue(any("architecture does not match" in error for error in errors))

    def test_firefox_cannot_be_a_fallback_assignment(self) -> None:
        roster = complete_roster()
        roster["fallback_assignments"][0]["browser"]["name"] = "firefox"
        errors = self.validate(roster)
        self.assertTrue(any("not the required Safari assignment" in error for error in errors))

    def test_roster_is_bound_to_candidate_identity(self) -> None:
        roster = complete_roster()
        roster["release"]["version"] = "0.2.7"
        self.assertTrue(any("differs from the candidate" in error for error in self.validate(roster)))


if __name__ == "__main__":
    unittest.main()
