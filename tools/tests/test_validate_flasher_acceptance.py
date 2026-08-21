from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "release" / "validate-flasher-acceptance.py"
SPEC = importlib.util.spec_from_file_location("validate_flasher_acceptance", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)

VERSION = "0.2.6"
SOURCE_COMMIT = "a" * 40
KEY_ID = "0123456789ABCDEF"
PUBLISHED_AT = "2026-07-20T12:00:00Z"
COMPLETED_AT = "2026-07-20T13:00:00Z"
MODELS = {
    "heltec-v4": "Heltec LoRa 32 V4 (S3R2)",
    "heltec-v4-r8": "Heltec LoRa 32 V4 (S3R8)",
    "t-beam-supreme": "LilyGO T-Beam Supreme",
    "xiao-esp32-c6": "Seeed XIAO ESP32-C6",
    "t-echo": "LilyGO T-Echo",
    "t114": "Heltec Mesh Node T114",
    "t096": "Heltec Mesh Node T096",
    "t1000-e": "Seeed SenseCAP T1000-E",
}


def manifest() -> dict:
    targets = []
    for board, model in MODELS.items():
        esp = board in {
            "heltec-v4",
            "heltec-v4-r8",
            "t-beam-supreme",
            "xiao-esp32-c6",
        }
        uf2 = board in {"t-echo", "t114", "t096"}
        chip = "esp32s3" if board in {"heltec-v4", "heltec-v4-r8", "t-beam-supreme"} else "esp32c6"
        targets.append(
            {
                "board_slug": board,
                "display_name": model,
                "transport": (
                    "esp-serial"
                    if esp
                    else "uf2-mass-storage"
                    if uf2
                    else "nrf-serial-dfu"
                ),
                "expected_chip": chip if esp else None,
                "parts": [{"path": f"{board}.bin", "size": 1, "sha256": "a" * 64}]
                if esp
                else [],
                "variants": (
                    []
                    if esp
                    else [
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
                    if board == "t-echo"
                    else [
                        {
                            "softdevice_family": "s140",
                            "softdevice_version": "6.1.1",
                            "fwid": "0x00b6",
                            "application_base": "0x00026000",
                            "family_id": "0xada52840",
                            "path": (
                                "heltec-t114-s140-6.1.1.uf2"
                                if board == "t114"
                                else "t096-s140-6.1.1.uf2"
                            ),
                            "size": 512,
                            "sha256": "d" * 64,
                        }
                    ]
                    if uf2
                    else []
                ),
                "nrf_serial_dfu": (
                    {
                        "application": {"kind": "dfu-application", "path": "t1000e.bin"},
                        "init_packet": {"kind": "dfu-init-packet", "path": "t1000e.dat"},
                        "recovery": {"artifact": {"kind": "uf2", "path": "t1000e.uf2"}},
                    }
                    if board == "t1000-e"
                    else None
                ),
                "provisioning": {"format": "HSPCFG1"}
                if board in {"heltec-v4", "heltec-v4-r8", "t-beam-supreme"}
                else None,
            }
        )
    return {
        "schema": 3,
        "release": {"version": VERSION, "channel": "stable", "commit": SOURCE_COMMIT},
        "signing": {"key_id": KEY_ID},
        "targets": targets,
    }


def evidence(root: Path, label: str) -> dict:
    content = label.encode("utf-8")
    digest = hashlib.sha256(content).hexdigest()
    (root / digest).write_bytes(content)
    return {
        "reference": f"artifact://qualification/{digest}",
        "sha256": digest,
        "redaction": {
            "reviewer": "fixture-reviewer",
            "credentials_removed": True,
            "device_identifiers_removed": True,
            "local_paths_removed": True,
            "private_network_data_removed": True,
        },
    }


def complete_acceptance(
    manifest_document: dict,
    manifest_path: Path,
    signature_path: Path,
    signed_bundle_path: Path,
    evidence_root: Path,
    roster: dict,
) -> dict:
    targets = {target["board_slug"]: target for target in manifest_document["targets"]}
    chip_counts = VALIDATOR.Counter(
        target["expected_chip"]
        for target in targets.values()
        if target["transport"] == "esp-serial"
    )
    runs = []
    for roster_assignment in roster["physical_assignments"]:
        board = roster_assignment["board"]
        surface = roster_assignment["surface"]
        target = targets[board]
        os_name = roster_assignment["os"]
        compatibilities = VALIDATOR.required_compatibilities(target)
        for compatibility in compatibilities:
            run = {
                "board": board,
                "surface": surface,
                "os": os_name,
                "architecture": roster_assignment["architecture"],
                "os_version": f"{os_name}-fixture-1",
                "hardware_identity": f"lab-{board}-01",
                "hardware_model": MODELS[board],
                "hardware_revision": "not-marked",
                "client": {
                    "name": "prns-web-flasher" if surface == "web" else "hopspot-flash",
                    "version": VERSION,
                },
                "scenarios": {
                    name: "pass"
                    for name in VALIDATOR.applicable_scenarios(
                        target, surface, chip_counts
                    )
                },
                "result": "pass",
                "tester": roster_assignment["tester"],
                "completed_at": COMPLETED_AT,
                "evidence": evidence(
                    evidence_root,
                    f"evidence://{board}/{surface}/{compatibility or 'default'}",
                ),
            }
            if compatibility is not None:
                run["compatibility_variant"] = compatibility
            if surface == "web":
                run["browser"] = {
                    **roster_assignment["browser"],
                    "version": "126.0.1",
                }
            runs.append(run)

    web_serial_smoke = []
    for roster_assignment in roster["web_serial_assignments"]:
        board = roster_assignment["board"]
        os_name = roster_assignment["os"]
        web_serial_smoke.append(
            {
                "board": board,
                "os": os_name,
                "architecture": roster_assignment["architecture"],
                "os_version": f"{os_name}-fixture-1",
                "hardware_identity": f"lab-{board}-firefox-01",
                "hardware_model": MODELS[board],
                "hardware_revision": "not-marked",
                "client": {"name": "prns-web-flasher", "version": VERSION},
                "browser": {
                    "name": "firefox",
                    "channel": "stable",
                    "version": "126.0.1",
                },
                "scenarios": {
                    name: "pass" for name in VALIDATOR.WEB_SERIAL_SCENARIOS
                },
                "result": "pass",
                "tester": roster_assignment["tester"],
                "completed_at": COMPLETED_AT,
                "evidence": evidence(
                    evidence_root, f"evidence://web-serial/{os_name}/{board}"
                ),
            }
        )

    browser_fallbacks = []
    for roster_assignment in roster["fallback_assignments"]:
        browser = roster_assignment["browser"]["name"]
        os_name = roster_assignment["os"]
        browser_fallbacks.append(
            {
                "os": os_name,
                "architecture": roster_assignment["architecture"],
                "os_version": f"{os_name}-fixture-1",
                "client": {"name": "prns-web-flasher", "version": VERSION},
                "browser": {
                    "name": browser,
                    "channel": "stable",
                    "version": "126.0.1",
                },
                "scenarios": {
                    name: "pass" for name in VALIDATOR.FALLBACK_SCENARIOS
                },
                "result": "pass",
                "tester": roster_assignment["tester"],
                "completed_at": COMPLETED_AT,
                "evidence": evidence(evidence_root, f"evidence://fallback/{browser}/{os_name}"),
            }
        )

    installation_smoke = []
    for roster_assignment in roster["installation_assignments"]:
        target = roster_assignment["target"]
        os_name = roster_assignment["os"]
        target_architecture = roster_assignment["architecture"]
        installation_smoke.append(
            {
                "target": target,
                "os": os_name,
                "architecture": target_architecture,
                "os_version": f"{os_name}-fixture-1",
                "cli_version": VERSION,
                "scenarios": {"install": "pass", "version": "pass"},
                "result": "pass",
                "tester": roster_assignment["tester"],
                "completed_at": COMPLETED_AT,
                "evidence": evidence(evidence_root, f"evidence://install/{target}"),
            }
        )

    return {
        "schema": 5,
        "candidate": {
            "version": VERSION,
            "channel": "stable",
            "source_commit": SOURCE_COMMIT,
            "signing_key_id": KEY_ID,
            "manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
            "manifest_signature_sha256": hashlib.sha256(signature_path.read_bytes()).hexdigest(),
            "signed_candidate_sha256": hashlib.sha256(signed_bundle_path.read_bytes()).hexdigest(),
            "prerelease_published_at": PUBLISHED_AT,
        },
        "runs": runs,
        "web_serial_smoke": web_serial_smoke,
        "browser_fallbacks": browser_fallbacks,
        "installation_smoke": installation_smoke,
    }


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
    for (board, surface), (os_name, target_architecture) in hosts.items():
        assignment = {
            "board": board,
            "surface": surface,
            "os": os_name,
            "architecture": target_architecture,
            "tester": "github:solo-fixture",
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
        "release": {"version": VERSION},
        "release_owner": "github:release-owner",
        "confirmed_on": "2026-07-20",
        "physical_assignments": physical_assignments,
        "web_serial_assignments": [
            {
                "board": board,
                "os": os_name,
                "architecture": target_architecture,
                "browser": {"name": "firefox", "channel": "stable"},
                "tester": "github:solo-fixture",
                "cables_ready": True,
                "device_permissions_ready": True,
                "recovery_instructions_reviewed": True,
            }
            for board, os_name, target_architecture in (
                ("heltec-v4", "linux", "x86_64"),
                ("t-beam-supreme", "macos", "x86_64"),
                ("xiao-esp32-c6", "windows", "x86_64"),
            )
        ],
        "fallback_assignments": [
            {
                "browser": {"name": browser, "channel": "stable"},
                "os": os_name,
                "architecture": target_architecture,
                "tester": "github:solo-fixture",
                "browser_ready": True,
            }
            for browser, os_name, target_architecture in (("safari", "macos", "aarch64"),)
        ],
        "installation_assignments": [
            {
                "target": target,
                "os": os_name,
                "architecture": target_architecture,
                "tester": "github:solo-fixture",
                "archive_ready": True,
            }
            for target, (os_name, target_architecture) in VALIDATOR.CLI_TARGETS.items()
        ],
    }


class AcceptanceValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.manifest_document = manifest()
        self.manifest_path = self.root / "flash-manifest.json"
        self.signature_path = self.root / "flash-manifest.json.minisig"
        self.signed_bundle_path = self.root / "prns-flasher-0.2.6-signed.tar.gz"
        self.roster_path = self.root / "tester-roster.json"
        self.evidence_root = self.root / "qualification-evidence"
        self.evidence_root.mkdir()
        self.roster = complete_roster()
        self.manifest_path.write_text(
            json.dumps(self.manifest_document, sort_keys=True) + "\n", encoding="utf-8"
        )
        self.signature_path.write_text("fixture signature\n", encoding="utf-8")
        self.signed_bundle_path.write_bytes(b"exact signed fixture candidate\n")
        self.roster_path.write_text(
            json.dumps(self.roster, sort_keys=True) + "\n", encoding="utf-8"
        )
        self.acceptance = complete_acceptance(
            self.manifest_document,
            self.manifest_path,
            self.signature_path,
            self.signed_bundle_path,
            self.evidence_root,
            self.roster,
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
                signed_bundle=self.signed_bundle_path,
                tester_roster=self.roster_path,
                evidence_root=self.evidence_root,
                prerelease_published_at=PUBLISHED_AT,
            ),
            now=datetime(2026, 7, 20, 14, 0, tzinfo=timezone.utc),
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
        self.assertTrue(any("is missing applicable scenarios" in error for error in errors))
        self.assertTrue(any("failed-sync" in error and "reboot-detection" in error for error in errors))

    def test_t1000e_requires_direct_dfu_and_recovery_boundaries(self) -> None:
        web = self.runs("t1000-e", "web")[0]
        web["scenarios"].pop("managed-application-entry")
        web["scenarios"].pop("recovery-uf2-fallback")
        errors = self.validate()
        self.assertTrue(any("is missing applicable scenarios" in error for error in errors))
        self.assertTrue(
            any(
                "managed-application-entry" in error
                and "recovery-uf2-fallback" in error
                for error in errors
            )
        )

    def test_t_echo_missing_compatibility_row_is_rejected(self) -> None:
        run = self.runs("t-echo", "web")[-1]
        self.acceptance["runs"].remove(run)
        (self.evidence_root / run["evidence"]["sha256"]).unlink()
        self.assertTrue(
            any(
                "missing board/surface/compatibility runs" in error
                for error in self.validate()
            )
        )

    def test_t_echo_compatibility_rows_cannot_reuse_evidence(self) -> None:
        first, second = self.runs("t-echo", "cli")
        old_digest = second["evidence"]["sha256"]
        second["evidence"] = dict(first["evidence"])
        (self.evidence_root / old_digest).unlink()
        self.assertTrue(
            any("reuses T-Echo compatibility evidence" in error for error in self.validate())
        )

    def test_esp_web_requires_device_md5_mismatch(self) -> None:
        for run in self.runs("heltec-v4", "web"):
            run["scenarios"].pop("device-md5-mismatch")
        self.assertTrue(
            any(
                "is missing applicable scenarios: ['device-md5-mismatch']" in error
                for error in self.validate()
            )
        )

    def test_same_chip_confirmation_is_required_only_for_shared_chip(self) -> None:
        for run in self.runs("t-beam-supreme", "cli"):
            run["scenarios"].pop("same-chip-board-confirmation")
        errors = self.validate()
        self.assertTrue(any("same-chip-board-confirmation" in error for error in errors))

    def test_non_provisioning_board_cannot_claim_configuration(self) -> None:
        self.runs("xiao-esp32-c6", "cli")[0]["scenarios"]["configure"] = "pass"
        self.assertTrue(
            any("claims scenarios that do not apply: ['configure']" in error for error in self.validate())
        )

    def test_firefox_web_serial_smokes_require_exact_os_coverage(self) -> None:
        removed = self.acceptance["web_serial_smoke"].pop(0)
        (self.evidence_root / removed["evidence"]["sha256"]).unlink()
        self.assertTrue(
            any("missing Firefox Web Serial smokes: ['linux']" in error for error in self.validate())
        )

    def test_firefox_web_serial_rejects_uf2_board_and_unsupported_page_evidence(self) -> None:
        smoke = self.acceptance["web_serial_smoke"][0]
        smoke["board"] = "t-echo"
        smoke["hardware_model"] = MODELS["t-echo"]
        smoke["scenarios"] = {
            name: "pass" for name in VALIDATOR.FALLBACK_SCENARIOS
        }
        errors = self.validate()
        self.assertTrue(any("cannot use a UF2 or unsupported board" in error for error in errors))
        self.assertTrue(any("claims scenarios that do not apply" in error for error in errors))

    def test_firefox_web_serial_requires_roster_hardware_and_unique_evidence(self) -> None:
        first, second = self.acceptance["web_serial_smoke"][:2]
        old_digest = second["evidence"]["sha256"]
        second["board"] = "heltec-v4-r8"
        second["hardware_model"] = "generic ESP board"
        second["evidence"] = dict(first["evidence"])
        (self.evidence_root / old_digest).unlink()
        errors = self.validate()
        self.assertTrue(any("board or host differs" in error for error in errors))
        self.assertTrue(any("hardware_model differs" in error for error in errors))
        self.assertTrue(any("reuses Firefox Web Serial evidence" in error for error in errors))

    def test_firefox_web_serial_requires_stable_exact_browser_and_all_scenarios(self) -> None:
        smoke = self.acceptance["web_serial_smoke"][0]
        smoke["browser"]["channel"] = "beta"
        smoke["browser"]["version"] = "Firefox/126"
        smoke["client"]["version"] = "0.2.5"
        smoke["scenarios"].pop("permission-grant")
        errors = self.validate()
        self.assertTrue(any("browser channel must be stable" in error for error in errors))
        self.assertTrue(any("must record exact firefox browser version" in error for error in errors))
        self.assertTrue(any("exact candidate version" in error for error in errors))
        self.assertTrue(any("missing Firefox Web Serial scenarios" in error for error in errors))

    def test_firefox_web_serial_rejects_duplicate_unsupported_host_and_failure(self) -> None:
        first, second = self.acceptance["web_serial_smoke"][:2]
        second["os"] = first["os"]
        second["architecture"] = "riscv64"
        second["result"] = "fail"
        second["hardware_identity"] = "NOT_RUN"
        errors = self.validate()
        self.assertTrue(any("duplicate Firefox Web Serial smoke" in error for error in errors))
        self.assertTrue(any("architecture does not match" in error for error in errors))
        self.assertTrue(any("not a passing Firefox Web Serial smoke" in error for error in errors))
        self.assertTrue(any("placeholder" in error for error in errors))

    def test_firefox_cannot_appear_as_a_fallback(self) -> None:
        self.acceptance["browser_fallbacks"][0]["browser"]["name"] = "firefox"
        errors = self.validate()
        self.assertTrue(any("not the required Safari fallback" in error for error in errors))

    def test_candidate_and_hardware_identity_must_match_signed_manifest(self) -> None:
        self.acceptance["candidate"]["source_commit"] = "b" * 40
        self.runs("heltec-v4", "web")[0]["hardware_model"] = "generic ESP board"
        errors = self.validate()
        self.assertTrue(any("source_commit does not match" in error for error in errors))
        self.assertTrue(any("hardware_model differs" in error for error in errors))

    def test_tampered_signed_candidate_bundle_is_rejected(self) -> None:
        self.signed_bundle_path.write_bytes(b"tampered signed fixture candidate\n")
        self.assertTrue(
            any("signed_candidate_sha256 does not match" in error for error in self.validate())
        )

    def test_every_physical_row_must_prove_fresh_install_and_post_flash_boot(self) -> None:
        run = self.runs("heltec-v4", "web")[0]
        run["scenarios"].pop("post-flash-boot")
        errors = self.validate()
        self.assertTrue(
            any("is missing applicable scenarios: ['post-flash-boot']" in error for error in errors)
        )

    def test_browser_must_be_stable_channel(self) -> None:
        self.runs("heltec-v4", "web")[0]["browser"]["channel"] = "beta"
        self.assertTrue(any("browser channel must be stable" in error for error in self.validate()))

    def test_browser_version_must_be_canonical_numeric_dotted_form(self) -> None:
        self.runs("heltec-v4", "web")[0]["browser"]["version"] = "Chrome/126.0.1"
        self.acceptance["browser_fallbacks"][0]["browser"]["version"] = "126"
        errors = self.validate()
        self.assertGreaterEqual(
            sum("must record exact" in error and "browser version" in error for error in errors),
            2,
        )

    def test_placeholders_and_unreviewed_evidence_fail_closed(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        run["tester"] = "TBD"
        run["evidence"] = {
            "reference": "REPLACE_WITH_LINK",
            "sha256": "NOT_RUN",
            "redaction": {
                "reviewer": "TBD",
                "credentials_removed": False,
                "device_identifiers_removed": False,
                "local_paths_removed": False,
                "private_network_data_removed": False,
            },
        }
        errors = self.validate()
        self.assertTrue(any("placeholder" in error for error in errors))
        self.assertTrue(any("redaction checks are not complete" in error for error in errors))

    def test_evidence_requires_strict_digest_reference_and_redaction(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        run["evidence"]["sha256"] = "A" * 64
        run["evidence"]["reference"] = "https://user:password@example.com/latest/log.txt?token=secret"
        run["evidence"]["redaction"]["credentials_removed"] = False
        errors = self.validate()
        self.assertTrue(any("artifact://qualification/LOWERCASE_SHA256" in error for error in errors))
        self.assertTrue(any("evidence sha256" in error for error in errors))
        self.assertTrue(any("credentials_removed" in error for error in errors))

    def test_malformed_evidence_reference_fails_without_crashing(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        run["evidence"]["reference"] = "https://[malformed/" + run["evidence"]["sha256"]
        self.assertTrue(
            any("artifact://qualification/LOWERCASE_SHA256" in error for error in self.validate())
        )

    def test_bogus_tester_not_assigned_to_host_is_rejected(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        run["tester"] = "github:unassigned-person"
        self.assertTrue(
            any("differs from the exact signed tester roster" in error for error in self.validate())
        )

    def test_missing_evidence_object_is_rejected(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        (self.evidence_root / run["evidence"]["sha256"]).unlink()
        self.assertTrue(
            any("evidence object is missing" in error for error in self.validate())
        )

    def test_mismatched_evidence_bytes_are_rejected(self) -> None:
        run = self.runs("heltec-v4", "cli")[0]
        (self.evidence_root / run["evidence"]["sha256"]).write_bytes(b"tampered evidence")
        self.assertTrue(
            any("evidence bytes do not match" in error for error in self.validate())
        )

    def test_unreferenced_evidence_object_is_rejected(self) -> None:
        content = b"unreferenced reviewed object"
        digest = hashlib.sha256(content).hexdigest()
        (self.evidence_root / digest).write_bytes(content)
        self.assertTrue(
            any("contains unreferenced objects" in error for error in self.validate())
        )

    def test_completion_before_prerelease_publication_is_rejected(self) -> None:
        self.runs("heltec-v4", "cli")[0]["completed_at"] = "2026-07-20T11:59:59Z"
        self.assertTrue(
            any("predates the exact public prerelease" in error for error in self.validate())
        )

    def test_cli_run_cannot_claim_browser_evidence(self) -> None:
        self.runs("heltec-v4", "cli")[0]["browser"] = {
            "name": "chrome",
            "channel": "stable",
            "version": "126.0.1",
        }
        self.assertTrue(any("CLI run must not claim browser evidence" in error for error in self.validate()))

    def test_installation_smoke_requires_install_and_exact_version(self) -> None:
        self.acceptance["installation_smoke"][0]["scenarios"].pop("version")
        self.assertTrue(
            any(
                "must prove both install and exact version" in error
                for error in self.validate()
            )
        )

    def test_browser_fallback_requires_every_truthful_fallback_scenario(self) -> None:
        self.acceptance["browser_fallbacks"][0]["scenarios"].pop("no-broken-connect-action")
        self.assertTrue(
            any("is missing fallback scenarios: ['no-broken-connect-action']" in error for error in self.validate())
        )

    def test_unknown_fields_and_future_timestamps_are_rejected(self) -> None:
        run = self.runs("heltec-v4", "web")[0]
        run["serial_number"] = "secret-device-serial"
        run["completed_at"] = "9999-12-31T23:59:59Z"
        errors = self.validate()
        self.assertTrue(any("unknown fields: ['serial_number']" in error for error in errors))
        self.assertTrue(any("completed_at cannot be in the future" in error for error in errors))

    def test_malformed_identity_fields_fail_without_crashing(self) -> None:
        self.acceptance["runs"][0]["board"] = {"not": "a string"}
        self.acceptance["browser_fallbacks"][0]["browser"]["name"] = ["safari"]
        self.acceptance["installation_smoke"][0]["target"] = {"not": "a target"}
        errors = self.validate()
        self.assertTrue(any("must be strings" in error for error in errors))
        self.assertTrue(any("not the required Safari fallback" in error for error in errors))
        self.assertTrue(any("unknown published target" in error for error in errors))

    def test_maintainer_override_cannot_replace_the_physical_matrix(self) -> None:
        record = {
            "schema": 5,
            "candidate": self.acceptance["candidate"],
            "maintainer_override": {
                "basis": "continuous development testing",
                "approved_by": "github:release-owner",
                "approved_at": COMPLETED_AT,
            },
        }
        errors = self.validate(record)
        self.assertTrue(any("unknown fields: ['maintainer_override']" in error for error in errors))
        self.assertTrue(any("acceptance runs must be an array" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
