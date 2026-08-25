from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "tools" / "release"
import sys

sys.path.insert(0, str(SCRIPTS))

from flasher_hotfix import compose, parse_spec, verify_candidate  # noqa: E402


BASE_VERSION = "0.3.7"
HOTFIX_VERSION = "0.3.7-hotfix.1"
BASE_COMMIT = "a" * 40
BASE_RECORD = "b" * 64
BASE_BUNDLE = "c" * 64


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def esp_target(board: str, version: str, payload: bytes) -> dict:
    return {
        "board_slug": board,
        "display_name": board,
        "transport": "esp-serial",
        "parts": [
            {
                "kind": "application",
                "path": f"firmware/hopspot/{board}/{version}/application.bin",
                "offset": 65536,
                "size": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        ],
        "variants": [],
        "provisioning": None,
    }


class HotfixCustodyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.repository = self.root / "repository"
        self.history = self.root / "history"
        self.candidate = self.root / "candidate"
        self.repository.mkdir()
        self.candidate.mkdir()
        (self.repository / "VERSION").write_text(f"{BASE_VERSION}\n", encoding="utf-8")

        self.base_payloads = {
            "heltec-v4": b"base-v4",
            "heltec-v4-r8": b"base-r8",
            "t-beam-supreme": b"base-tbeam",
        }
        self.base_manifest = {
            "schema": 3,
            "release": {
                "version": BASE_VERSION,
                "channel": "stable",
                "commit": BASE_COMMIT,
            },
            "signing": {"key_id": "0123456789ABCDEF"},
            "targets": [
                esp_target(board, BASE_VERSION, payload)
                for board, payload in self.base_payloads.items()
            ],
        }
        base_root = self.history / "releases" / BASE_VERSION
        write_json(base_root / "flash-manifest.json", self.base_manifest)
        (base_root / "flash-manifest.json.minisig").write_text(
            "fixture signature\n", encoding="utf-8"
        )
        for target in self.base_manifest["targets"]:
            artifact = target["parts"][0]
            path = base_root / artifact["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(self.base_payloads[target["board_slug"]])
        base_manifest_hash = digest(base_root / "flash-manifest.json")
        write_json(
            self.history / "history.json",
            {
                "schema": 1,
                "mode": "retained",
                "head": {
                    "version": BASE_VERSION,
                    "source_commit": BASE_COMMIT,
                    "manifest_sha256": base_manifest_hash,
                    "release_record_sha256": BASE_RECORD,
                    "signed_bundle_sha256": BASE_BUNDLE,
                },
            },
        )
        self.spec = {
            "schema": 1,
            "release": {
                "version": HOTFIX_VERSION,
                "base_version": BASE_VERSION,
                "base_source_commit": BASE_COMMIT,
                "base_manifest_sha256": base_manifest_hash,
                "base_release_record_sha256": BASE_RECORD,
                "base_signed_candidate_sha256": BASE_BUNDLE,
            },
            "changed_boards": ["heltec-v4"],
            "qualification": {
                "physical_boards": ["heltec-v4"],
                "deferred_hardware": [],
                "surfaces": ["web"],
                "required_scenarios": ["fresh-install", "post-flash-boot"],
                "required_checks": ["tcp-client-enabled-boot"],
            },
            "summary": "Fixture hotfix with one freshly built board target.",
        }
        spec_path = (
            self.repository
            / "release"
            / "flash"
            / "hotfixes"
            / f"{HOTFIX_VERSION}.json"
        )
        write_json(spec_path, self.spec)

        self.write_changed_board("heltec-v4", b"hotfix-v4")

    def write_changed_board(self, board: str, payload: bytes) -> None:
        changed = esp_target(board, HOTFIX_VERSION, payload)
        changed_root = (
            self.candidate
            / "firmware"
            / "hopspot"
            / board
            / HOTFIX_VERSION
        )
        write_json(changed_root / "target.json", changed)
        write_json(
            changed_root / "source-capability.json",
            {
                "schema": 1,
                "board_slug": board,
                "nominally_capable": False,
                "status": "absent",
                "source": None,
                "reserve_bytes": None,
            },
        )
        (changed_root / "application.bin").write_bytes(payload)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def finish_candidate(self) -> None:
        targets = []
        for board in self.base_payloads:
            record = (
                self.candidate
                / "firmware"
                / "hopspot"
                / board
                / HOTFIX_VERSION
                / "target.json"
            )
            targets.append(json.loads(record.read_text(encoding="utf-8")))
        write_json(
            self.candidate / "flash-manifest.json",
            {
                "schema": 3,
                "release": {
                    "version": HOTFIX_VERSION,
                    "channel": "stable",
                    "commit": "d" * 40,
                },
                "signing": {"key_id": "0123456789ABCDEF"},
                "targets": targets,
            },
        )
        (self.candidate / "VERSION").write_text(f"{HOTFIX_VERSION}\n", encoding="utf-8")
        embedded_spec = self.candidate / "qualification" / "hotfix.json"
        embedded_spec.parent.mkdir(parents=True)
        shutil.copy2(
            self.repository
            / "release"
            / "flash"
            / "hotfixes"
            / f"{HOTFIX_VERSION}.json",
            embedded_spec,
        )
        base_destination = (
            self.candidate / "website" / "releases" / BASE_VERSION
        )
        shutil.copytree(self.history / "releases" / BASE_VERSION, base_destination)

    def test_compose_inherits_every_unaffected_byte_and_verifies_scope(self) -> None:
        metadata = compose(
            self.repository, self.history, self.candidate, HOTFIX_VERSION
        )
        self.assertEqual(metadata["changed_boards"], ["heltec-v4"])
        self.assertEqual(
            metadata["inherited_boards"], ["heltec-v4-r8", "t-beam-supreme"]
        )
        inherited = (
            self.candidate
            / "firmware"
            / "hopspot"
            / "heltec-v4-r8"
            / HOTFIX_VERSION
            / "application.bin"
        )
        self.assertEqual(inherited.read_bytes(), self.base_payloads["heltec-v4-r8"])
        self.finish_candidate()
        verified = verify_candidate(self.repository, self.candidate)
        self.assertIsNotNone(verified)
        self.assertEqual(verified.changed_boards, ("heltec-v4",))

    def test_inherited_byte_tampering_is_rejected(self) -> None:
        compose(self.repository, self.history, self.candidate, HOTFIX_VERSION)
        self.finish_candidate()
        inherited = (
            self.candidate
            / "firmware"
            / "hopspot"
            / "heltec-v4-r8"
            / HOTFIX_VERSION
            / "application.bin"
        )
        inherited.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "differs from its manifest"):
            verify_candidate(self.repository, self.candidate)

    def test_spec_rejects_declaring_every_shipping_board_changed(self) -> None:
        self.spec["changed_boards"] = [
            "heltec-v4",
            "heltec-v4-r8",
            "t-beam-supreme",
        ]
        path = self.root / f"{HOTFIX_VERSION}.json"
        write_json(path, self.spec)
        with self.assertRaisesRegex(ValueError, "strict subset"):
            parse_spec(
                path, {"heltec-v4", "heltec-v4-r8", "t-beam-supreme"}
            )

    def test_every_declared_changed_board_must_change_an_artifact(self) -> None:
        self.spec["changed_boards"] = ["heltec-v4", "heltec-v4-r8"]
        self.spec["qualification"]["physical_boards"] = [
            "heltec-v4",
            "heltec-v4-r8",
        ]
        write_json(
            self.repository
            / "release"
            / "flash"
            / "hotfixes"
            / f"{HOTFIX_VERSION}.json",
            self.spec,
        )
        self.write_changed_board("heltec-v4-r8", self.base_payloads["heltec-v4-r8"])
        compose(self.repository, self.history, self.candidate, HOTFIX_VERSION)
        self.finish_candidate()
        with self.assertRaisesRegex(ValueError, "heltec-v4-r8"):
            verify_candidate(self.repository, self.candidate)

    def test_spec_records_physical_scope_and_an_explicit_hardware_deferral(self) -> None:
        self.spec["changed_boards"] = ["heltec-v4", "heltec-v4-r8"]
        self.spec["qualification"]["deferred_hardware"] = [
            {
                "board": "heltec-v4-r8",
                "basis": "The changed implementation is shared with the physically checked target.",
                "follow_up": "Capture and review a post-flash R8 boot log after promotion.",
            }
        ]
        path = self.root / f"{HOTFIX_VERSION}.json"
        write_json(path, self.spec)
        parsed = parse_spec(
            path, {"heltec-v4", "heltec-v4-r8", "t-beam-supreme"}
        )
        self.assertEqual(parsed.physical_boards, ("heltec-v4",))
        self.assertEqual(
            [deferral.board for deferral in parsed.deferred_hardware],
            ["heltec-v4-r8"],
        )

    def test_later_hotfix_may_inherit_an_earlier_hotfix(self) -> None:
        self.spec["release"]["version"] = "0.3.7-hotfix.2"
        self.spec["release"]["base_version"] = HOTFIX_VERSION
        path = (
            self.repository
            / "release"
            / "flash"
            / "hotfixes"
            / "0.3.7-hotfix.2.json"
        )
        write_json(path, self.spec)
        parsed = parse_spec(path, {"heltec-v4", "heltec-v4-r8"})
        self.assertEqual(parsed.roster_version, BASE_VERSION)
        self.assertEqual(parsed.base_version, HOTFIX_VERSION)
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPTS / "flasher_hotfix.py"),
                "identity",
                "--repository",
                str(self.repository),
                "--version",
                "0.3.7-hotfix.2",
                "--format",
                "roster-version",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.stdout.strip(), BASE_VERSION)


if __name__ == "__main__":
    unittest.main()
