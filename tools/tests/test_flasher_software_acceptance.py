from __future__ import annotations

import argparse
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import flasher_software_acceptance as software
from flasher_acceptance_contract import software_qualification
from flasher_tester_roster import validate_roster
from test_create_flasher_acceptance import CREATOR, VALIDATOR, manifest

NOW = datetime(2026, 10, 4, 12, tzinfo=timezone.utc)
COMMIT = "a" * 40


def readiness():
    runner, inventory = software.validation_contract()
    results = {}
    for suite in runner.selected_suites(inventory, [], None, "release"):
        results[suite["id"]] = {
            "schema": 1, "suite": suite["id"], "domain": suite["domain"],
            "commit": COMMIT, "platform": "linux" if suite["platform"] == "any" else suite["platform"],
            "required_platform": suite["platform"], "worktree_clean": True,
            "command": ["fixture-test"], "tool_versions": {"fixture": "1"},
            "started_at": "2026-10-03T01:00:00Z", "finished_at": "2026-10-03T02:00:00Z",
            "duration_seconds": 3600, "status": "passed", "exit_code": 0,
            "timed_out": False, "spawn_error": None,
        }
    return {"schema": 1, "commit": COMMIT, "tier": "release", "generated_at": "2026-10-03T03:00:00Z", "results": results}


class SoftwareAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.evidence = readiness()
        self.manifest = manifest()
        self.manifest["release"].update(version="0.3.8", commit=COMMIT)
        self.roster = {"schema": 5, "release": {"version": "0.3.8"}, "release_owner": "github:owner", "confirmed_on": "2026-10-03"}
        self.args = argparse.Namespace(
            manifest=root / "flash-manifest.json", manifest_signature=root / "manifest.minisig",
            signed_bundle=root / "signed.tar.gz", tester_roster=root / "tester-roster.json",
            prerelease_published_at="2026-10-04T01:00:00Z", output=root / "acceptance.json",
            acceptance=root / "acceptance.json", evidence_root=root / "evidence",
            readiness_manifest=root / "readiness.json", readiness_run_id="12345",
        )
        self.args.manifest.write_text(json.dumps(self.manifest))
        self.args.manifest_signature.write_bytes(b"signature fixture")
        self.args.signed_bundle.write_bytes(b"candidate fixture")
        self.args.tester_roster.write_text(json.dumps(self.roster))
        self.args.readiness_manifest.write_text(json.dumps(self.evidence))
        self.args.evidence_root.mkdir()

    def create(self):
        with patch.object(CREATOR, "verify_hotfix_candidate", return_value=None):
            CREATOR.create(self.args)
        digest = hashlib.sha256(self.args.readiness_manifest.read_bytes()).hexdigest()
        (self.args.evidence_root / digest).write_bytes(self.args.readiness_manifest.read_bytes())
        return json.loads(self.args.output.read_text())

    def test_bundled_gate_uses_candidate_version_and_committed_inventory(self):
        repository = Path(__file__).resolve().parents[2]
        root = Path(self.temp.name) / "candidate"
        qualification = root / "qualification"
        qualification.mkdir(parents=True)
        for source, destination in (("validation/run.py", "validation_runner.py"),
                                    ("validation/manifest.toml", "validation-manifest.toml")):
            (qualification / destination).write_bytes((repository / source).read_bytes())
        for version in ("0.3.8", "0.3.8-hotfix.1", "0.3.9"):
            (root / "VERSION").write_text(version + "\n")
            with self.subTest(version=version), patch.object(software, "__file__", str(qualification / "flasher_software_acceptance.py")):
                runner, inventory = software.validation_contract()
                suites = runner.selected_suites(inventory, [], None, "release")
                proofs = {suite["id"] for suite in suites if suite["domain"] == "kani"}
                self.assertEqual(proofs, set() if version == "0.3.8" else {
                    "kani-" + proof["name"] for proof in inventory["kani"]
                })
                self.assertIn("core-work-simulation-extended", {suite["id"] for suite in suites})
                errors = software.readiness_errors(self.evidence, COMMIT, NOW)
                self.assertEqual(bool(errors), version != "0.3.8")

    def test_complete_automated_release_needs_no_physical_rows(self):
        record = self.create()
        self.assertEqual(record["schema"], 7)
        self.assertEqual(record["physical_qualification"], "not-required-pre-1.0")
        self.assertNotIn("runs", record)
        self.assertEqual(VALIDATOR.validate(self.args, NOW), [])

    def test_runner_timestamps_round_trip_without_rewriting_evidence(self):
        for suffix in [".123456+00:00", ".123456Z", "+00:00"]:
            with self.subTest(suffix=suffix):
                document = deepcopy(self.evidence)
                document["generated_at"] = document["generated_at"][:-1] + suffix
                for result in document["results"].values():
                    for field in ["started_at", "finished_at"]:
                        result[field] = result[field][:-1] + suffix
                original = json.dumps(document).encode()
                self.args.readiness_manifest.write_bytes(original)
                self.args.output.unlink(missing_ok=True)
                record = self.create()
                self.assertEqual(VALIDATOR.validate(self.args, NOW), [])
                self.assertEqual(self.args.readiness_manifest.read_bytes(), original)
                self.assertEqual(record["software_validation"]["sha256"], hashlib.sha256(original).hexdigest())
                (self.args.evidence_root / record["software_validation"]["sha256"]).unlink()

    def test_readiness_ordering_retains_fractional_precision(self):
        document = deepcopy(self.evidence)
        document["generated_at"] = "2026-10-03T03:00:00.100000+00:00"
        document["results"]["virtual-device-simulation"]["finished_at"] = "2026-10-03T03:00:00.200000+00:00"
        self.assertIn("virtual-device-simulation: result finishes after evidence generation", software.readiness_errors(document, COMMIT, NOW))

    def test_readiness_timestamps_require_valid_explicit_utc(self):
        for value in [None, "2026-10-03T03:00:00", "2026-10-03T03:00:00+01:00", "2026-10-03T03:00:00-01:00", "2026-10-03", "2026-02-30T03:00:00+00:00"]:
            for field in ["generated_at", "finished_at"]:
                with self.subTest(value=value, field=field):
                    document = deepcopy(self.evidence)
                    if field == "generated_at":
                        document[field] = value
                    else:
                        document["results"]["virtual-device-simulation"][field] = value
                    self.assertTrue(software.readiness_errors(document, COMMIT, NOW))

    def test_automated_hotfix_uses_its_suite_release_owner(self):
        self.manifest["release"]["version"] = "0.3.8-hotfix.1"
        self.args.manifest.write_text(json.dumps(self.manifest))
        spec = argparse.Namespace(roster_version="0.3.8")
        with patch.object(CREATOR, "verify_hotfix_candidate", return_value=spec):
            CREATOR.create(self.args)
        digest = hashlib.sha256(self.args.readiness_manifest.read_bytes()).hexdigest()
        (self.args.evidence_root / digest).write_bytes(self.args.readiness_manifest.read_bytes())
        with patch.object(VALIDATOR, "verify_hotfix_candidate", return_value=spec):
            self.assertEqual(VALIDATOR.validate(self.args, NOW), [])
        self.assertTrue(VALIDATOR.validate(self.args, NOW))

    def test_policy_has_explicit_version_boundary(self):
        for version in ["0.3.8", "0.3.8-hotfix.1", "0.4.0", "0.99.9"]:
            self.assertTrue(software_qualification(version), version)
        for version in [None, "next", "0.3.7", "0.3.7-hotfix.9", "1.0.0", "10.3.8", "0.03.8"]:
            self.assertFalse(software_qualification(version), version)

    def test_missing_failed_and_wrong_source_results_are_rejected(self):
        for name in ["virtual-device-simulation", "embedded-isa-thumbv7em", "embedded-platform-nrf52840", "embedded-platform-esp32s3"]:
            for mutation in ["missing", "failed", "commit", "dirty", "timed_out", "future", "tools"]:
                with self.subTest(suite=name, mutation=mutation):
                    document = deepcopy(self.evidence)
                    result = document["results"][name]
                    if mutation == "missing": del document["results"][name]
                    elif mutation == "failed": result["status"] = "failed"
                    elif mutation == "commit": result["commit"] = "b" * 40
                    elif mutation == "dirty": result["worktree_clean"] = False
                    elif mutation == "timed_out": result["timed_out"] = True
                    elif mutation == "future": result["finished_at"] = "2026-10-05T01:00:00Z"
                    elif mutation == "tools": result["tool_versions"] = {}
                    self.assertTrue(software.readiness_errors(document, COMMIT, NOW))

    def test_partial_tier_and_extra_suites_are_rejected(self):
        for change in ["tier", "commit", "extra", "future", "malformed"]:
            document = deepcopy(self.evidence)
            if change == "tier": document["tier"] = "pr"
            elif change == "commit": document["commit"] = "b" * 40
            elif change == "extra": document["results"]["unknown"] = {}
            elif change == "future": document["generated_at"] = "2026-10-05T00:00:00Z"
            else: document["results"]["virtual-device-simulation"] = []
            self.assertTrue(software.readiness_errors(document, COMMIT, NOW), change)

    def test_physical_claim_and_unreferenced_or_tampered_evidence_are_rejected(self):
        original = self.create()
        for change in ["claim", "run", "extra", "tampered", "candidate"]:
            record = deepcopy(original)
            object_path = self.args.evidence_root / record["software_validation"]["sha256"]
            object_path.write_bytes(self.args.readiness_manifest.read_bytes())
            extra = self.args.evidence_root / ("f" * 64)
            extra.unlink(missing_ok=True)
            if change == "claim": record["physical_qualification"] = "passed"
            elif change == "run": record["software_validation"]["workflow_run_id"] = "../123"
            elif change == "extra": extra.write_bytes(b"unexpected")
            elif change == "tampered": object_path.write_bytes(b"modified")
            else: record["candidate"]["signed_candidate_sha256"] = "f" * 64
            self.args.acceptance.write_text(json.dumps(record))
            self.assertTrue(VALIDATOR.validate(self.args, NOW), change)

    def test_new_release_cannot_fall_back_to_physical_or_override_schema(self):
        record = self.create()
        for schema in [4, 5, 6]:
            record["schema"] = schema
            self.args.acceptance.write_text(json.dumps(record))
            self.assertIn("schema-7 automated acceptance", " ".join(VALIDATOR.validate(self.args, NOW)))

    def test_software_roster_cannot_waive_historical_or_stable_release(self):
        self.assertEqual(validate_roster(self.roster, "0.3.8")[1], [])
        for version in ["0.3.7", "1.0.0"]:
            self.roster["release"]["version"] = version
            self.assertTrue(validate_roster(self.roster, version)[1])

    def test_generator_requires_actual_readiness_evidence(self):
        self.args.readiness_manifest = None
        with self.assertRaisesRegex(ValueError, "readiness-manifest"):
            self.create()


if __name__ == "__main__":
    unittest.main()
