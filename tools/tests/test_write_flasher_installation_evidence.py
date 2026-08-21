from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1] / "release"
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))
PATH = SCRIPTS / "write-flasher-installation-evidence.py"
SPEC = importlib.util.spec_from_file_location("write_flasher_installation_evidence", PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {PATH}")
WRITER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WRITER)


def roster(version: str) -> dict:
    physical = []
    hosts = (
        ("heltec-v4", "cli", "linux", "x86_64"),
        ("heltec-v4", "web", "linux", "x86_64"),
        ("heltec-v4-r8", "cli", "linux", "x86_64"),
        ("heltec-v4-r8", "web", "linux", "x86_64"),
        ("t-beam-supreme", "cli", "macos", "aarch64"),
        ("t-beam-supreme", "web", "macos", "aarch64"),
        ("xiao-esp32-c6", "cli", "windows", "x86_64"),
        ("xiao-esp32-c6", "web", "windows", "x86_64"),
        ("t-echo", "cli", "macos", "aarch64"),
        ("t-echo", "web", "linux", "x86_64"),
        ("t114", "cli", "linux", "x86_64"),
        ("t114", "web", "macos", "aarch64"),
        ("t096", "cli", "linux", "aarch64"),
        ("t096", "web", "macos", "aarch64"),
        ("t1000-e", "cli", "macos", "x86_64"),
        ("t1000-e", "web", "windows", "x86_64"),
    )
    for board, surface, os_name, architecture in hosts:
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
        physical.append(assignment)
    fallbacks = [
        {
            "browser": {"name": browser, "channel": "stable"},
            "os": os_name,
            "architecture": architecture,
            "tester": "github:solo-tester",
            "browser_ready": True,
        }
        for browser, os_name, architecture in (("safari", "macos", "aarch64"),)
    ]
    installations = [
        {
            "target": target,
            "os": os_name,
            "architecture": architecture,
            "tester": "github:solo-tester",
            "archive_ready": True,
        }
        for target, (os_name, architecture) in {
            "aarch64-apple-darwin": ("macos", "aarch64"),
            "x86_64-apple-darwin": ("macos", "x86_64"),
            "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
            "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
            "x86_64-pc-windows-msvc": ("windows", "x86_64"),
        }.items()
    ]
    return {
        "schema": 3,
        "release": {"version": version},
        "release_owner": "github:release-owner",
        "confirmed_on": "2026-07-24",
        "physical_assignments": physical,
        "web_serial_assignments": [
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
        ],
        "fallback_assignments": fallbacks,
        "installation_assignments": installations,
    }


class InstallationEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.version = "0.3.0"
        self.target = "x86_64-unknown-linux-gnu"
        self.archive = (
            self.root
            / f"hopspot-flash-{self.version}-{self.target}.tar.gz"
        )
        self.archive.write_bytes(b"published archive")
        self.roster = self.root / "tester-roster.json"
        self.roster.write_text(
            json.dumps(roster(self.version)) + "\n",
            encoding="utf-8",
        )
        self.output = self.root / "evidence.json"
        self.arguments = argparse.Namespace(
            roster=self.roster,
            version=self.version,
            target=self.target,
            source_commit="a" * 40,
            signed_candidate_sha256="b" * 64,
            published_at="2026-07-24T12:00:00Z",
            archive=self.archive,
            expected_archive_sha256=hashlib.sha256(
                self.archive.read_bytes()
            ).hexdigest(),
            version_output="hopspot-flash 0.3.0",
            os_version="Ubuntu 24.04.3 LTS",
            repository="owner/repository",
            workflow_run_id="42",
            workflow_run_attempt="2",
            workflow_job="install (x86_64-unknown-linux-gnu)",
            workflow_sha="a" * 40,
            completed_at="2026-07-24T12:05:00Z",
            output=self.output,
        )
        self.now = datetime(2026, 7, 24, 12, 10, tzinfo=timezone.utc)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_writes_roster_bound_exact_archive_evidence(self) -> None:
        evidence = WRITER.create(self.arguments, now=self.now)
        self.assertEqual(
            evidence,
            json.loads(self.output.read_text(encoding="utf-8")),
        )
        self.assertEqual(
            evidence["assignment"],
            {
                "target": self.target,
                "os": "linux",
                "architecture": "x86_64",
                "tester": "github:solo-tester",
            },
        )
        self.assertEqual(
            evidence["observations"],
            {
                "install": "pass",
                "version": "pass",
                "version_output": "hopspot-flash 0.3.0",
                "os_version": "Ubuntu 24.04.3 LTS",
            },
        )

    def test_rejects_wrong_archive_bytes(self) -> None:
        self.arguments.expected_archive_sha256 = "c" * 64
        with self.assertRaisesRegex(
            ValueError,
            "archive bytes differ",
        ):
            WRITER.create(self.arguments, now=self.now)

    def test_rejects_wrong_installed_version(self) -> None:
        self.arguments.version_output = "hopspot-flash 0.2.8"
        with self.assertRaisesRegex(
            ValueError,
            "reported a different version",
        ):
            WRITER.create(self.arguments, now=self.now)

    def test_rejects_prepublication_observation(self) -> None:
        self.arguments.completed_at = "2026-07-24T11:59:59Z"
        with self.assertRaisesRegex(
            ValueError,
            "predates public release publication",
        ):
            WRITER.create(self.arguments, now=self.now)

    def test_rejects_non_repository_identity(self) -> None:
        self.arguments.repository = "repository-without-owner"
        with self.assertRaisesRegex(
            ValueError,
            "owner/name identity",
        ):
            WRITER.create(self.arguments, now=self.now)


if __name__ == "__main__":
    unittest.main()
