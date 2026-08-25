from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1] / "release"
ROOT = SCRIPTS.parents[1]


def load_script(name: str, module_name: str):
    path = SCRIPTS / name
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not import {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CREATOR = load_script("create-flasher-acceptance.py", "create_flasher_acceptance")
VALIDATOR = load_script("validate-flasher-acceptance.py", "validate_flasher_acceptance_scaffold")
CONTRACT = load_script("flasher_acceptance_contract.py", "flasher_acceptance_contract_test")
PUBLISHED_AT = "2026-07-20T12:00:00Z"


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
    return {
        "schema": 3,
        "release": {"version": "0.2.6"},
        "release_owner": "github:release-owner",
        "confirmed_on": "2026-07-20",
        "physical_assignments": physical_assignments,
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
        "fallback_assignments": [
            {
                "browser": {"name": browser, "channel": "stable"},
                "os": os_name,
                "architecture": architecture,
                "tester": "github:solo-tester",
                "browser_ready": True,
            }
            for browser, os_name, architecture in (("safari", "macos", "aarch64"),)
        ],
        "installation_assignments": [
            {
                "target": target,
                "os": os_name,
                "architecture": architecture,
                "tester": "github:solo-tester",
                "archive_ready": True,
            }
            for target, (os_name, architecture) in CONTRACT.CLI_TARGETS.items()
        ],
    }


def manifest() -> dict:
    boards = (
        ("heltec-v4", "Heltec LoRa 32 V4 (S3R2)", "esp-serial", "esp32s3", True),
        ("heltec-v4-r8", "Heltec LoRa 32 V4 (S3R8)", "esp-serial", "esp32s3", True),
        ("t-beam-supreme", "LilyGO T-Beam Supreme", "esp-serial", "esp32s3", True),
        ("xiao-esp32-c6", "Seeed XIAO ESP32-C6", "esp-serial", "esp32c6", False),
        ("t-echo", "LilyGO T-Echo", "uf2-mass-storage", None, False),
        ("t114", "Heltec Mesh Node T114", "uf2-mass-storage", None, False),
        ("t096", "Heltec Mesh Node T096", "uf2-mass-storage", None, False),
        ("t1000-e", "Seeed SenseCAP T1000-E", "nrf-serial-dfu", None, False),
    )
    return {
        "schema": 3,
        "release": {"version": "0.2.6", "channel": "preview", "commit": "a" * 40},
        "signing": {"key_id": "0123456789ABCDEF"},
        "targets": [
            {
                "board_slug": slug,
                "display_name": display_name,
                "transport": transport,
                "expected_chip": chip,
                "parts": (
                    [{"path": f"{slug}.bin", "size": 1, "sha256": "a" * 64}]
                    if transport == "esp-serial"
                    else []
                ),
                "variants": (
                    [
                        {
                            "softdevice_family": "s140",
                            "softdevice_version": version,
                            "fwid": fwid,
                            "application_base": application_base,
                            "family_id": "0xada52840",
                            "path": f"t-echo-s140-{version}.uf2",
                            "size": 512,
                            "sha256": digest,
                        }
                        for version, fwid, application_base, digest in (
                            ("6.1.1", "0x00b6", "0x00026000", "b" * 64),
                            ("7.3.0", "0x0123", "0x00027000", "c" * 64),
                        )
                    ]
                    if slug == "t-echo"
                    else [
                        {
                            "softdevice_family": "s140",
                            "softdevice_version": "6.1.1",
                            "fwid": "0x00b6",
                            "application_base": "0x00026000",
                            "family_id": "0xada52840",
                            "path": "heltec-t114-s140-6.1.1.uf2",
                            "size": 512,
                            "sha256": "d" * 64,
                        }
                    ]
                    if slug == "t114"
                    else [
                        {
                            "softdevice_family": "s140",
                            "softdevice_version": "6.1.1",
                            "fwid": "0x00b6",
                            "application_base": "0x00026000",
                            "family_id": "0xada52840",
                            "path": "t096-s140-6.1.1.uf2",
                            "size": 512,
                            "sha256": "e" * 64,
                        }
                    ]
                    if slug == "t096"
                    else []
                ),
                "nrf_serial_dfu": (
                    {
                        "application": {"kind": "dfu-application", "path": "t1000e.bin"},
                        "init_packet": {"kind": "dfu-init-packet", "path": "t1000e.dat"},
                        "recovery": {"artifact": {"kind": "uf2", "path": "t1000e.uf2"}},
                    }
                    if slug == "t1000-e"
                    else None
                ),
                "provisioning": {"format": "HSPCFG1"} if provisioned else None,
            }
            for slug, display_name, transport, chip, provisioned in boards
        ],
    }


class AcceptanceScaffoldTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.manifest_path = self.root / "flash-manifest.json"
        self.signature_path = self.root / "flash-manifest.json.minisig"
        self.signed_bundle_path = self.root / "prns-flasher-0.2.6-signed.tar.gz"
        self.output_path = self.root / "acceptance.json"
        self.roster_path = self.root / "tester-roster.json"
        self.evidence_root = self.root / "qualification-evidence"
        self.evidence_root.mkdir()
        self.manifest_document = manifest()
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        self.signature_path.write_text("fixture minisign\n", encoding="utf-8")
        self.signed_bundle_path.write_bytes(b"exact signed candidate fixture\n")
        self.roster_path.write_text(
            json.dumps(complete_roster()) + "\n", encoding="utf-8"
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def create(self) -> dict:
        CREATOR.create(
            argparse.Namespace(
                manifest=self.manifest_path,
                manifest_signature=self.signature_path,
                signed_bundle=self.signed_bundle_path,
                tester_roster=self.roster_path,
                prerelease_published_at=PUBLISHED_AT,
                output=self.output_path,
            )
        )
        return json.loads(self.output_path.read_text(encoding="utf-8"))

    def test_scaffold_binds_exact_candidate_and_never_claims_pass(self) -> None:
        record = self.create()
        candidate = record["candidate"]
        self.assertEqual(
            candidate["signed_candidate_sha256"],
            hashlib.sha256(self.signed_bundle_path.read_bytes()).hexdigest(),
        )
        self.assertEqual(candidate["prerelease_published_at"], PUBLISHED_AT)
        self.assertEqual(record["schema"], 5)
        self.assertEqual(len(record["runs"]), 18)
        self.assertEqual(len(record["web_serial_smoke"]), 3)
        self.assertEqual(len(record["browser_fallbacks"]), 1)
        self.assertEqual(len(record["installation_smoke"]), 5)
        self.assertEqual(
            {smoke["os"] for smoke in record["web_serial_smoke"]},
            set(CONTRACT.WEB_SERIAL_HOSTS),
        )
        self.assertTrue(
            all(
                smoke["board"] in CONTRACT.ESP_SERIAL_BOARDS
                and smoke["browser"]["name"] == "firefox"
                and set(smoke["scenarios"]) == CONTRACT.WEB_SERIAL_SCENARIOS
                for smoke in record["web_serial_smoke"]
            )
        )
        self.assertEqual(
            record["browser_fallbacks"][0]["browser"]["name"],
            "safari",
        )
        self.assertTrue(
            all(
                smoke["scenarios"] == {
                    "install": "not-run",
                    "version": "not-run",
                }
                for smoke in record["installation_smoke"]
            )
        )
        encoded = json.dumps(record)
        self.assertNotIn('"pass"', encoded)
        self.assertIn('"not-run"', encoded)

    def test_prospective_templates_advance_without_rewriting_published_custody(self) -> None:
        acceptance_template = json.loads(
            (ROOT / "release" / "acceptance" / "template.json").read_text(
                encoding="utf-8"
            )
        )
        roster_template = json.loads(
            (ROOT / "release" / "acceptance" / "roster-template.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(acceptance_template["schema"], 5)
        self.assertEqual(roster_template["schema"], 3)
        records = ROOT / "release" / "acceptance" / "records"
        rosters = ROOT / "release" / "acceptance" / "rosters"
        self.assertEqual(
            {
                json.loads(path.read_text(encoding="utf-8"))["schema"]
                for path in records.glob("0.3.*.json")
            },
            {4, 6},
        )
        self.assertEqual(
            {
                json.loads(path.read_text(encoding="utf-8"))["schema"]
                for path in rosters.glob("0.3.*.json")
            },
            {2, 3},
        )

    def test_scaffold_assigns_complete_transport_aware_coverage(self) -> None:
        record = self.create()
        targets = {
            target["board_slug"]: target for target in self.manifest_document["targets"]
        }
        chip_counts = Counter(
            target["expected_chip"]
            for target in targets.values()
            if target["transport"] == "esp-serial"
        )
        for board, target in targets.items():
            for surface in CONTRACT.SURFACES:
                rows = [
                    run
                    for run in record["runs"]
                    if run["board"] == board and run["surface"] == surface
                ]
                expected_compatibilities = CONTRACT.required_compatibilities(target)
                self.assertEqual(len(rows), len(expected_compatibilities))
                self.assertEqual(
                    {row.get("compatibility_variant") for row in rows},
                    set(expected_compatibilities),
                )
                for row in rows:
                    self.assertEqual(
                        set(row["scenarios"]),
                        CONTRACT.applicable_scenarios(target, surface, chip_counts),
                    )

    def test_hotfix_scaffold_contains_only_committed_changed_board_checks(self) -> None:
        self.manifest_document["release"]["version"] = "0.2.6-hotfix.1"
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        roster, errors = VALIDATOR.validate_roster(complete_roster(), "0.2.6")
        self.assertEqual(errors, [])
        spec = CONTRACT.HotfixSpec(
            path=self.root / "0.2.6-hotfix.1.json",
            version="0.2.6-hotfix.1",
            base_version="0.2.6",
            base_source_commit="b" * 40,
            base_manifest_sha256="c" * 64,
            base_release_record_sha256="d" * 64,
            base_signed_candidate_sha256="e" * 64,
            changed_boards=("heltec-v4",),
            physical_boards=("heltec-v4",),
            deferred_hardware=(),
            surfaces=("web",),
            required_scenarios=("fresh-install", "post-flash-boot"),
            required_checks=("tcp-client-enabled-boot",),
            summary="Fixture target-scoped firmware hotfix qualification.",
        )
        record = CONTRACT.hotfix_scaffold(
            self.manifest_document,
            self.manifest_path,
            self.signature_path,
            self.signed_bundle_path,
            PUBLISHED_AT,
            roster,
            spec,
        )
        self.assertEqual(record["schema"], 6)
        self.assertEqual(
            [(run["board"], run["surface"]) for run in record["runs"]],
            [("heltec-v4", "web")],
        )
        self.assertEqual(
            record["runs"][0]["checks"], {"tcp-client-enabled-boot": "not-run"}
        )

    def test_unperformed_scaffold_fails_closed_in_validator(self) -> None:
        self.create()
        errors = VALIDATOR.validate(
            argparse.Namespace(
                acceptance=self.output_path,
                manifest=self.manifest_path,
                manifest_signature=self.signature_path,
                signed_bundle=self.signed_bundle_path,
                tester_roster=self.roster_path,
                evidence_root=self.evidence_root,
                prerelease_published_at=PUBLISHED_AT,
            )
        )
        self.assertTrue(errors)
        self.assertTrue(any("not a passing acceptance run" in error for error in errors))

    def test_existing_output_is_never_overwritten(self) -> None:
        self.output_path.write_text("preserve me\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
            self.create()
        self.assertEqual(self.output_path.read_text(encoding="utf-8"), "preserve me\n")

    def test_failed_temporary_creation_removes_only_its_reservation(self) -> None:
        with patch.object(CREATOR.tempfile, "mkstemp", side_effect=OSError("disk unavailable")):
            with self.assertRaisesRegex(OSError, "disk unavailable"):
                self.create()
        self.assertFalse(self.output_path.exists())

    def test_duplicate_or_malformed_manifest_targets_are_rejected(self) -> None:
        duplicate = dict(self.manifest_document["targets"][0])
        self.manifest_document["targets"].append(duplicate)
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(ValueError, "well-formed targets"):
            self.create()

    def test_t_echo_requires_both_exact_compatibility_variants(self) -> None:
        t_echo = next(
            target
            for target in self.manifest_document["targets"]
            if target["board_slug"] == "t-echo"
        )
        t_echo["variants"].pop()
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(ValueError, "exact S140 v6 and v7"):
            self.create()

    def test_t096_requires_its_exact_pinned_compatibility_variant(self) -> None:
        t096 = next(
            target
            for target in self.manifest_document["targets"]
            if target["board_slug"] == "t096"
        )
        t096["variants"][0]["softdevice_version"] = "7.3.0"
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(ValueError, "t096 acceptance requires its exact pinned"):
            self.create()

    def test_prerelease_publication_requires_full_utc_timestamp(self) -> None:
        with self.assertRaisesRegex(ValueError, "full UTC timestamp"):
            CREATOR.create(
                argparse.Namespace(
                    manifest=self.manifest_path,
                    manifest_signature=self.signature_path,
                    signed_bundle=self.signed_bundle_path,
                    tester_roster=self.roster_path,
                    prerelease_published_at="2026-07-20",
                    output=self.output_path,
                )
            )


if __name__ == "__main__":
    unittest.main()
