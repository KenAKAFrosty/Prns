from __future__ import annotations

import copy
import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


RUNNER_PATH = Path(__file__).resolve().parents[1] / "run.py"
SPEC = importlib.util.spec_from_file_location("validation_runner", RUNNER_PATH)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class RegistryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = runner.load_manifest()

    def test_duplicate_suite_ids_are_rejected(self) -> None:
        suite = copy.deepcopy(self.manifest["suite"][0])
        with self.assertRaisesRegex(runner.ValidationError, "duplicate suite id"):
            runner.suite_map({"suite": [suite, suite]})

    def test_invalid_tier_and_platform_are_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["suite"][0]["tiers"] = ["eventually"]
        manifest["suite"][0]["platform"] = "templeos"
        errors = runner.validate_manifest(manifest)
        self.assertTrue(any("tiers must contain" in error for error in errors))
        self.assertTrue(any("invalid platform" in error for error in errors))

    def test_missing_input_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["suite"][0]["inputs"] = ["validation/does-not-exist"]
        errors = runner.validate_manifest(manifest)
        self.assertTrue(any("input is missing" in error for error in errors))

    def test_cargo_manifest_discovery_uses_the_git_source_inventory(self) -> None:
        git_sources = [runner.ROOT / "Cargo.toml"]
        with mock.patch.object(
            runner, "tracked_or_untracked_sources", return_value=git_sources
        ):
            self.assertEqual(runner.source_cargo_manifests(), {"Cargo.toml"})

    def test_unregistered_interop_asset_is_rejected(self) -> None:
        orphan = runner.ROOT / "validation/interop/peers/runner_self_test_orphan.py"
        orphan.write_text("# temporary registry self-test\n", encoding="utf-8")
        try:
            errors = runner.validate_manifest(copy.deepcopy(self.manifest))
            self.assertTrue(any("unregistered validation assets" in error for error in errors))
        finally:
            orphan.unlink()

    def test_malformed_mutation_triage_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "triage.toml"
            path.write_text(
                """schema = 1
[[accepted]]
fingerprint = "not-a-digest"
reason = ""
reviewer = ""
expires = "yesterday"
""",
                encoding="utf-8",
            )
            errors = runner.validate_triage(path)
        self.assertGreaterEqual(len(errors), 4)

    def test_mutant_fingerprint_ignores_source_coordinates(self) -> None:
        mutant = {
            "package": "prns-core",
            "file": "prns-core/src/wire.rs",
            "function": {
                "function_name": "parse",
                "return_type": "-> Result<Packet, Error>",
                "span": {"start": {"line": 10, "column": 2}},
            },
            "genre": "FnValue",
            "replacement": "Err(Default::default())",
            "name": "prns-core/src/wire.rs:11:3: replace parse -> Result<Packet, Error>",
            "span": {"start": {"line": 11, "column": 3}},
        }
        moved = copy.deepcopy(mutant)
        moved["function"]["span"]["start"]["line"] = 410
        moved["span"]["start"]["line"] = 411
        moved["name"] = "prns-core/src/wire.rs:411:3: replace parse -> Result<Packet, Error>"
        self.assertEqual(runner.mutation_fingerprint(mutant), runner.mutation_fingerprint(moved))

    def test_timeout_writes_structured_evidence(self) -> None:
        suite = {
            "id": "runner-timeout-self-test",
            "domain": "hygiene",
            "tiers": ["pr"],
            "platform": "any",
            "toolchain": "python",
            "timeout_seconds": 1,
            "command": [sys.executable, "-c", "import time; time.sleep(10)"],
            "inputs": ["validation/run.py"],
            "artifacts": "validation-artifacts/results/runner-timeout-self-test",
        }
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.dict(os.environ, {"PRNS_VALIDATION_ARTIFACTS": directory}):
                self.assertFalse(runner.run_suite(self.manifest, suite, None, 1))
            result_path = Path(directory) / "results/runner-timeout-self-test/result.json"
            result = json.loads(result_path.read_text(encoding="utf-8"))
        self.assertEqual(result["schema"], 1)
        self.assertEqual(result["status"], "failed")
        self.assertTrue(result["timed_out"])
        self.assertEqual(result["commit"], runner.git_head())
        self.assertIn("rustc", result["tool_versions"])
        self.assertEqual(runner.evidence_errors(result), [])
        del result["finished_at"]
        self.assertTrue(any("missing fields" in error for error in runner.evidence_errors(result)))

    def test_ci_matrix_is_deterministic(self) -> None:
        first = json.dumps(
            {"include": runner.selected_suites(self.manifest, [], "kani", "release")},
            sort_keys=True,
        )
        second = json.dumps(
            {"include": runner.selected_suites(self.manifest, [], "kani", "release")},
            sort_keys=True,
        )
        self.assertEqual(first, second)
        identifiers = [entry["id"] for entry in json.loads(first)["include"]]
        self.assertEqual(identifiers, sorted(identifiers))

    def test_physical_android_is_separate_from_hosted_release_readiness(self) -> None:
        release = runner.selected_suites(self.manifest, [], None, "release")
        scheduled = runner.selected_suites(self.manifest, [], None, "scheduled")
        release_ids = {suite["id"] for suite in release}
        scheduled_ids = {suite["id"] for suite in scheduled}
        self.assertNotIn("android-runtime-device", release_ids)
        self.assertIn("android-runtime-device", scheduled_ids)
        self.assertNotIn(
            ["self-hosted", "linux", "android", "prns-release"],
            [entry["runner"] for entry in runner.ci_matrix(release)["include"]],
        )

    def test_platform_selection_is_explicit_and_host_aware(self) -> None:
        portable = runner.selected_suites(self.manifest, [], None, None, "any")
        self.assertTrue(portable)
        self.assertTrue(all(suite["platform"] == "any" for suite in portable))

        current = runner.selected_suites(self.manifest, [], None, None, "current")
        self.assertTrue(current)
        allowed = {"any", runner.native_platform()}
        self.assertTrue(all(suite["platform"] in allowed for suite in current))

        macos = runner.selected_suites(self.manifest, [], None, None, "macos")
        self.assertTrue(all(suite["platform"] == "macos" for suite in macos))

    def test_platform_selector_is_available_to_list_matrix_and_run(self) -> None:
        parser = runner.build_parser()
        for command in ("list", "matrix", "run"):
            arguments = parser.parse_args([command, "--platform", "current"])
            self.assertEqual(arguments.platform, "current")

    def test_nightly_toolchain_is_exact_and_resolves_every_suite_command(self) -> None:
        nightly = runner.named_toolchain(self.manifest, "nightly")
        self.assertEqual(nightly, "nightly-2025-11-21")
        parser = runner.build_parser()
        arguments = parser.parse_args(["toolchain", "nightly"])
        self.assertEqual(arguments.name, "nightly")
        commands = [
            part
            for suite in runner.virtual_suites(self.manifest)
            for part in suite["command"]
        ]
        self.assertNotIn(runner.NIGHTLY_TOOLCHAIN_ARGUMENT, commands)
        self.assertIn(f"+{nightly}", commands)

    def test_verification_report_explains_its_guarantees(self) -> None:
        report = "\n".join(runner.verification_report(self.manifest, check_tools=False))
        for guarantee in (
            "Suite policy",
            "Declared inputs",
            "Cargo ownership",
            "Native discovery",
            "Asset ownership",
            "External references",
            "Mutation policy",
        ):
            self.assertIn(guarantee, report)
        self.assertIn(f"{len(self.manifest['kani'])} Kani proofs", report)
        self.assertIn(f"{len(self.manifest['fuzz_target'])} fuzz targets", report)
        self.assertIn("pull-request", report)

    def test_cleanup_never_selects_corpora_or_runtime_state(self) -> None:
        selected = {path.relative_to(runner.ROOT).as_posix() for path in runner.cleanup_paths(self.manifest)}
        forbidden_fragments = ("/corpus", ".reticulum", "prnsd/.run", ".vscode", ".wifi-env")
        self.assertFalse(any(fragment in path for path in selected for fragment in forbidden_fragments))


if __name__ == "__main__":
    unittest.main()
