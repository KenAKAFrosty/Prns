from __future__ import annotations

import argparse
from copy import deepcopy
from datetime import date
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "validate-flasher-acceptance.py"
SPEC = importlib.util.spec_from_file_location("validate_flasher_acceptance", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)

VERSION = "0.2.6"
SOURCE_COMMIT = "a" * 40
KEY_ID = "0123456789ABCDEF"
MODELS = {
    "heltec-v4": "Heltec LoRa 32 V4",
    "t-beam-supreme": "LilyGO T-Beam Supreme",
    "xiao-esp32-c6": "Seeed XIAO ESP32-C6",
    "t-echo": "LilyGO T-Echo",
}


def manifest() -> dict:
    targets = []
    for board, model in MODELS.items():
        esp = board != "t-echo"
        chip = "esp32s3" if board in {"heltec-v4", "t-beam-supreme"} else "esp32c6"
        targets.append(
            {
                "board_slug": board,
                "display_name": model,
                "transport": "esp-serial" if esp else "uf2-mass-storage",
                "expected_chip": chip if esp else None,
                "provisioning": {"format": "HSPCFG1"}
                if board in {"heltec-v4", "t-beam-supreme"}
                else None,
            }
        )
    return {
        "schema": 2,
        "release": {"version": VERSION, "channel": "stable", "commit": SOURCE_COMMIT},
        "signing": {"key_id": KEY_ID},
        "targets": targets,
    }


def evidence(reference: str) -> dict:
    return {"reference": reference, "redaction": "reviewed"}


def architecture(board: str, os_name: str) -> str:
    if os_name == "macos":
        return "x86_64" if board == "t-beam-supreme" else "aarch64"
    if os_name == "linux":
        return "aarch64" if board == "xiao-esp32-c6" else "x86_64"
    return "x86_64"


def complete_acceptance(manifest_document: dict, manifest_path: Path, signature_path: Path) -> dict:
    targets = {target["board_slug"]: target for target in manifest_document["targets"]}
    chip_counts = VALIDATOR.Counter(
        target["expected_chip"]
        for target in targets.values()
        if target["transport"] == "esp-serial"
    )
    runs = []
    for board, target in targets.items():
        for surface in sorted(VALIDATOR.SURFACES):
            scenarios = {
                name: "pass"
                for name in VALIDATOR.applicable_scenarios(target, surface, chip_counts)
            }
            for os_name in ("macos", "windows", "linux"):
                run = {
                    "board": board,
                    "surface": surface,
                    "os": os_name,
                    "architecture": architecture(board, os_name),
                    "os_version": f"{os_name}-fixture-1",
                    "hardware_identity": f"lab-{board}-01",
                    "hardware_model": MODELS[board],
                    "hardware_revision": "not-marked",
                    "client": {
                        "name": "prns-web-flasher" if surface == "web" else "hopspot-flash",
                        "version": VERSION,
                    },
                    "scenarios": dict(scenarios),
                    "result": "pass",
                    "tester": "fixture-tester",
                    "date": date.today().isoformat(),
                    "evidence": evidence(f"evidence://{board}/{surface}/{os_name}"),
                }
                if surface == "web":
                    run["browser"] = {
                        "name": "edge" if os_name == "windows" else "chrome",
                        "version": "fixture-126.0.1",
                    }
                runs.append(run)

    fallback_architecture = {"macos": "aarch64", "linux": "x86_64", "windows": "x86_64"}
    browser_fallbacks = []
    for browser, os_name in sorted(VALIDATOR.REQUIRED_FALLBACKS):
        browser_fallbacks.append(
            {
                "os": os_name,
                "architecture": fallback_architecture[os_name],
                "os_version": f"{os_name}-fixture-1",
                "client": {"name": "prns-web-flasher", "version": VERSION},
                "browser": {"name": browser, "version": "fixture-126.0.1"},
                "result": "pass",
                "tester": "fixture-tester",
                "date": date.today().isoformat(),
                "evidence": evidence(f"evidence://fallback/{browser}/{os_name}"),
            }
        )

    installation_smoke = []
    for target, (os_name, target_architecture) in VALIDATOR.CLI_TARGETS.items():
        installation_smoke.append(
            {
                "target": target,
                "os": os_name,
                "architecture": target_architecture,
                "os_version": f"{os_name}-fixture-1",
                "cli_version": VERSION,
                "scenarios": {"install": "pass", "doctor": "pass"},
                "result": "pass",
                "tester": "fixture-tester",
                "date": date.today().isoformat(),
                "evidence": evidence(f"evidence://install/{target}"),
            }
        )

    return {
        "schema": 2,
        "candidate": {
            "version": VERSION,
            "channel": "stable",
            "source_commit": SOURCE_COMMIT,
            "signing_key_id": KEY_ID,
            "manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
            "manifest_signature_sha256": hashlib.sha256(signature_path.read_bytes()).hexdigest(),
        },
        "runs": runs,
        "browser_fallbacks": browser_fallbacks,
        "installation_smoke": installation_smoke,
    }


class AcceptanceValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.manifest_document = manifest()
        self.manifest_path = self.root / "flash-manifest.json"
        self.signature_path = self.root / "flash-manifest.json.minisig"
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        self.signature_path.write_text("fixture signature\n", encoding="utf-8")
        self.acceptance = complete_acceptance(
            self.manifest_document, self.manifest_path, self.signature_path
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def validate(self, acceptance: dict | None = None) -> list[str]:
        acceptance_path = self.root / "acceptance.json"
        acceptance_path.write_text(
            json.dumps(self.acceptance if acceptance is None else acceptance, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return VALIDATOR.validate(
            argparse.Namespace(
                acceptance=acceptance_path,
                manifest=self.manifest_path,
                manifest_signature=self.signature_path,
            )
        )

    def runs(self, board: str, surface: str) -> list[dict]:
        return [
            run
            for run in self.acceptance["runs"]
            if run["board"] == board and run["surface"] == surface
        ]

    def test_complete_transport_aware_record_passes(self) -> None:
        self.assertEqual(self.validate(), [])

    def test_t_echo_web_rejects_cli_device_claims(self) -> None:
        self.runs("t-echo", "web")[0]["scenarios"]["failed-sync"] = "pass"
        self.assertTrue(
            any("claims scenarios that do not apply: ['failed-sync']" in error for error in self.validate())
        )

    def test_t_echo_cli_requires_copy_sync_and_reboot_evidence(self) -> None:
        for run in self.runs("t-echo", "cli"):
            run["scenarios"].pop("failed-sync")
            run["scenarios"].pop("reboot-detection")
        errors = self.validate()
        self.assertTrue(any("t-echo/cli is missing scenarios" in error for error in errors))
        self.assertTrue(any("failed-sync" in error and "reboot-detection" in error for error in errors))

    def test_esp_web_requires_device_md5_mismatch(self) -> None:
        for run in self.runs("heltec-v4", "web"):
            run["scenarios"].pop("device-md5-mismatch")
        self.assertTrue(
            any(
                "heltec-v4/web is missing scenarios: ['device-md5-mismatch']" in error
                for error in self.validate()
            )
        )

    def test_same_chip_confirmation_is_required_only_for_shared_chip(self) -> None:
        for run in self.runs("t-beam-supreme", "cli"):
            run["scenarios"].pop("same-chip-board-confirmation")
        errors = self.validate()
        self.assertTrue(any("t-beam-supreme/cli is missing scenarios" in error for error in errors))
        self.assertFalse(any("xiao-esp32-c6" in error for error in errors))

    def test_non_provisioning_board_cannot_claim_configuration(self) -> None:
        self.runs("xiao-esp32-c6", "cli")[0]["scenarios"]["configure"] = "pass"
        self.assertTrue(
            any("claims scenarios that do not apply: ['configure']" in error for error in self.validate())
        )

    def test_fallbacks_are_independent_browser_evidence(self) -> None:
        self.acceptance["browser_fallbacks"] = [
            entry
            for entry in self.acceptance["browser_fallbacks"]
            if not (entry["browser"]["name"] == "firefox" and entry["os"] == "linux")
        ]
        self.assertTrue(
            any("missing browser fallback checks: [('firefox', 'linux')]" in error for error in self.validate())
        )

    def test_candidate_and_hardware_identity_must_match_signed_manifest(self) -> None:
        self.acceptance["candidate"]["source_commit"] = "b" * 40
        self.runs("heltec-v4", "web")[0]["hardware_model"] = "generic ESP board"
        errors = self.validate()
        self.assertTrue(any("source_commit does not match" in error for error in errors))
        self.assertTrue(any("hardware_model differs" in error for error in errors))

    def test_placeholders_and_unreviewed_evidence_fail_closed(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        run["tester"] = "TBD"
        run["evidence"] = {"reference": "REPLACE_WITH_LINK", "redaction": "pending"}
        errors = self.validate()
        self.assertTrue(any("placeholder" in error for error in errors))
        self.assertTrue(any("redaction must be 'reviewed'" in error for error in errors))

    def test_cli_run_cannot_claim_browser_evidence(self) -> None:
        self.runs("heltec-v4", "cli")[0]["browser"] = {
            "name": "chrome",
            "version": "126.0.1",
        }
        self.assertTrue(any("CLI run must not claim browser evidence" in error for error in self.validate()))

    def test_installation_smoke_requires_install_and_doctor(self) -> None:
        self.acceptance["installation_smoke"][0]["scenarios"].pop("doctor")
        self.assertTrue(any("must prove both install and doctor" in error for error in self.validate()))

    def test_unknown_fields_and_future_dates_are_rejected(self) -> None:
        run = self.runs("heltec-v4", "web")[0]
        run["serial_number"] = "secret-device-serial"
        run["date"] = "9999-12-31"
        errors = self.validate()
        self.assertTrue(any("unknown fields: ['serial_number']" in error for error in errors))
        self.assertTrue(any("date cannot be in the future" in error for error in errors))

    def test_malformed_identity_fields_fail_without_crashing(self) -> None:
        self.acceptance["runs"][0]["board"] = {"not": "a string"}
        self.acceptance["browser_fallbacks"][0]["browser"]["name"] = ["firefox"]
        self.acceptance["installation_smoke"][0]["target"] = {"not": "a target"}
        errors = self.validate()
        self.assertTrue(any("must be strings" in error for error in errors))
        self.assertTrue(any("not a required Safari/Firefox fallback" in error for error in errors))
        self.assertTrue(any("unknown published target" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
