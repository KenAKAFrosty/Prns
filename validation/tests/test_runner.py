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

EXACT_SHA = "a" * 40


def mutation_manifest(shards: int = 4) -> dict:
    return {
        "tools": {"cargo_mutants": "27.1.0"},
        "suite": [
            {
                "id": "mutation-analysis",
                "domain": "mutation",
                "tiers": ["release"],
                "platform": "linux",
                "toolchain": "stable",
                "timeout_seconds": 14400,
                "shards": shards,
                "command": ["bash", "validation/mutation/run.sh"],
                "inputs": ["validation/mutation/run.sh"],
                "artifacts": "validation-artifacts/results/mutation-analysis",
            }
        ],
    }


def mutation_payload() -> dict:
    return {
        "outcomes": [{"scenario": "Baseline", "summary": "Success"}],
        "total_mutants": 0,
        "missed": 0,
        "caught": 0,
        "timeout": 0,
        "unviable": 0,
        "success": 0,
        "start_time": "2026-07-26T00:00:00Z",
        "end_time": "2026-07-26T00:01:00Z",
        "cargo_mutants_version": "27.1.0",
    }


def mutation_evidence(suite: dict, commit: str = EXACT_SHA) -> dict:
    return {
        "schema": 1,
        "suite": suite["id"],
        "domain": suite["domain"],
        "commit": commit,
        "platform": "linux",
        "required_platform": suite["platform"],
        "worktree_clean": True,
        "command": suite["command"],
        "tool_versions": {
            "python": "Python 3.14.0",
            "cargo": "cargo 1.96.0",
            "rustc": "rustc 1.96.0",
            "cargo-mutants": "cargo-mutants 27.1.0",
        },
        "started_at": "2026-07-26T00:00:00+00:00",
        "finished_at": "2026-07-26T00:01:00+00:00",
        "duration_seconds": 60,
        "status": "passed",
        "exit_code": 0,
        "timed_out": False,
        "spawn_error": None,
        "shard": suite["shard"],
    }


def write_mutation_artifacts(
    root: Path,
    manifest: dict,
    *,
    omitted: set[str] | None = None,
    shard_overrides: dict[str, dict] | None = None,
    commit_overrides: dict[str, str] | None = None,
) -> None:
    omitted = omitted or set()
    shard_overrides = shard_overrides or {}
    commit_overrides = commit_overrides or {}
    for suite in runner.selected_suites(manifest, [], "mutation", "release"):
        if suite["id"] in omitted:
            continue
        evidence = mutation_evidence(
            suite, commit_overrides.get(suite["id"], EXACT_SHA)
        )
        if suite["id"] in shard_overrides:
            evidence["shard"] = shard_overrides[suite["id"]]
        result = root / "results" / suite["id"] / "result.json"
        result.parent.mkdir(parents=True)
        result.write_text(json.dumps(evidence), encoding="utf-8")
        outcomes = (
            root
            / "mutation"
            / suite["id"]
            / "mutants.out"
            / "outcomes.json"
        )
        outcomes.parent.mkdir(parents=True)
        outcomes.write_text(json.dumps(mutation_payload()), encoding="utf-8")


def run_mutation_aggregate(
    root: Path, manifest: dict, triage: str = "schema = 1\n"
) -> Path:
    triage_path = root / "triage.toml"
    triage_path.write_text(triage, encoding="utf-8")
    with (
        mock.patch.object(runner, "validate_manifest", return_value=[]),
        mock.patch.object(runner, "git_head", return_value=EXACT_SHA),
        mock.patch.object(runner, "tracked_worktree_is_clean", return_value=True),
        mock.patch.object(runner, "TRIAGE_PATH", triage_path),
        mock.patch.dict(os.environ, {"PRNS_VALIDATION_ARTIFACTS": str(root)}),
    ):
        return runner.aggregate(manifest, EXACT_SHA, "release", "mutation")


class RegistryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = runner.load_manifest()

    def test_duplicate_suite_ids_are_rejected(self) -> None:
        suite = copy.deepcopy(self.manifest["suite"][0])
        with self.assertRaisesRegex(runner.ValidationError, "duplicate suite id"):
            runner.suite_map({"suite": [suite, suite]})

    def test_manifest_schema_has_an_independent_version_owner(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.toml"
            path.write_text(
                f"schema = {runner.MANIFEST_SCHEMA + 1}\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                runner.ValidationError,
                f"validation manifest schema must be {runner.MANIFEST_SCHEMA}",
            ):
                runner.load_manifest(path)

    def test_mutation_triage_schema_has_an_independent_version_owner(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "triage.toml"
            path.write_text(
                f"schema = {runner.MUTATION_TRIAGE_SCHEMA + 1}\n",
                encoding="utf-8",
            )
            self.assertIn(
                f"mutation triage schema must be {runner.MUTATION_TRIAGE_SCHEMA}",
                runner.validate_triage(path),
            )

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

    def test_format_package_overrides_are_scoped_and_unique(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["registry"]["format_package_overrides"] = {
            "validation/not-a-format-root/Cargo.toml": ["missing-package"],
            "personal-hopspot/embedded/nrf52840/Cargo.toml": [
                "t-echo",
                "t-echo",
            ],
        }
        errors = runner.validate_manifest(manifest)
        self.assertTrue(
            any(
                "format package overrides name unknown manifests" in error
                for error in errors
            )
        )
        self.assertTrue(
            any("must contain unique package names" in error for error in errors)
        )

    def test_invalid_shard_definitions_are_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        mutation = next(
            suite for suite in manifest["suite"] if suite["id"] == "mutation-analysis"
        )
        mutation["shards"] = "4"
        mutation["command"].append("--shard")
        manifest["suite"][0]["shards"] = 4
        errors = runner.validate_manifest(manifest)
        self.assertTrue(any("shards must be an integer" in error for error in errors))
        self.assertTrue(any("may shard only the mutation domain" in error for error in errors))
        self.assertTrue(any("must derive shard arguments" in error for error in errors))

    def test_mutation_shards_are_round_robin_and_artifact_unique(self) -> None:
        self.assertEqual(
            runner.selected_suites(self.manifest, [], "mutation", "release"),
            [],
        )
        self.assertEqual(
            runner.selected_suites(self.manifest, [], "mutation", "pr"),
            [],
        )
        suites = runner.selected_suites(self.manifest, [], "mutation", "scheduled")
        self.assertEqual(len(suites), 4)
        self.assertEqual(len({suite["id"] for suite in suites}), 4)
        self.assertEqual(len({suite["artifacts"] for suite in suites}), 4)
        for index, suite in enumerate(suites):
            self.assertEqual(
                suite["command"][-4:],
                ["--shard", f"{index}/4", "--sharding", "round-robin"],
            )
            self.assertEqual(
                suite["shard"],
                {"suite": "mutation-analysis", "index": index, "total": 4},
            )

    def test_mutation_surface_executes_owning_core_tests(self) -> None:
        config = runner.load_toml(runner.ROOT / "validation/mutation/config.toml")
        arguments = config["additional_cargo_test_args"]
        packages = [
            arguments[index + 1]
            for index, argument in enumerate(arguments)
            if argument == "-p"
        ]
        self.assertEqual(packages, ["personal-rns", "prns-core"])
        self.assertIn("runtime-metrics", arguments[-1].split())

    def test_mutation_exclusions_are_not_source_coordinate_fragile(self) -> None:
        config = runner.load_toml(runner.ROOT / "validation/mutation/config.toml")
        for excluded in config["exclude_re"]:
            self.assertNotIn(".rs:", excluded)
            self.assertNotIn(r"\.rs:", excluded)

    def test_incomplete_mutation_shard_is_rejected(self) -> None:
        payload = mutation_payload()
        payload["end_time"] = None
        self.assertIn(
            "mutation run is incomplete",
            runner.mutation_results_errors(payload, "27.1.0"),
        )

    def test_mutation_completion_accepts_cargo_mutants_nanoseconds(self) -> None:
        payload = mutation_payload()
        payload["start_time"] = "2026-07-26T14:47:46.62648725Z"
        payload["end_time"] = "2026-07-26T14:56:35.861487512Z"
        self.assertEqual(runner.mutation_results_errors(payload, "27.1.0"), [])

    def test_runner_python_argument_resolves_to_the_current_interpreter(self) -> None:
        suite = {
            "id": "runner-python-self-test",
            "command": [runner.RUNNER_PYTHON_ARGUMENT, "-c", "pass"],
        }
        with tempfile.TemporaryDirectory() as directory:
            command = runner.command_for(suite, 1, Path(directory) / "results" / suite["id"])
        self.assertEqual(command, [sys.executable, "-c", "pass"])

    def test_interpreter_environment_path_derives_from_its_version(self) -> None:
        specification = {
            "version": "1.2.3",
            "venv": "validation/.venv/rns-{version}",
        }
        self.assertEqual(
            runner.interpreter_venv(specification),
            runner.ROOT / "validation/.venv/rns-1.2.3",
        )

    def test_runner_python_argument_is_rejected_outside_command_element_zero(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["suite"][0]["command"] = ["cargo", runner.RUNNER_PYTHON_ARGUMENT]
        errors = runner.validate_manifest(manifest)
        self.assertTrue(any("only as command element zero" in error for error in errors))

    def test_unresolved_python_argument_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["suite"][0]["command"] = ["prefix-__UNKNOWN_PYTHON__", "-c", "pass"]
        errors = runner.validate_manifest(manifest)
        self.assertTrue(any("unresolved Python argument" in error for error in errors))

    def test_windows_entrypoint_runs_cargo_without_a_shell(self) -> None:
        entrypoint = runner.ROOT / "validation/platforms/windows.py"
        spec = importlib.util.spec_from_file_location("windows_validation", entrypoint)
        assert spec is not None and spec.loader is not None
        windows = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(windows)
        with (
            mock.patch.object(windows.subprocess, "run") as run,
            mock.patch("builtins.print"),
        ):
            windows.main()
        self.assertEqual(
            [call.args[0] for call in run.call_args_list],
            list(windows.COMMANDS),
        )
        self.assertTrue(
            all(call.kwargs["cwd"] == windows.ROOT for call in run.call_args_list)
        )
        self.assertTrue(all(call.kwargs["check"] is True for call in run.call_args_list))
        self.assertTrue(
            all(
                "bash" not in command and "shell" not in call.kwargs
                for command, call in zip(windows.COMMANDS, run.call_args_list)
            )
        )

    def test_cargo_manifest_discovery_uses_the_git_source_inventory(self) -> None:
        git_sources = [runner.ROOT / "Cargo.toml"]
        with mock.patch.object(
            runner, "tracked_or_untracked_sources", return_value=git_sources
        ):
            self.assertEqual(runner.source_cargo_manifests(), {"Cargo.toml"})

    def test_cargo_lock_registry_exactly_owns_first_party_workspaces(self) -> None:
        self.assertEqual(
            set(self.manifest["registry"]["cargo_lock_workspaces"]),
            runner.source_cargo_lock_workspaces(),
        )

    def test_cargo_lock_discovery_excludes_vendored_upstream_locks(self) -> None:
        sources = [
            runner.ROOT / "Cargo.lock",
            runner.ROOT / "first-party" / "Cargo.lock",
            runner.ROOT / "first-party" / "vendor" / "upstream" / "Cargo.lock",
        ]
        with mock.patch.object(runner, "tracked_or_untracked_sources", return_value=sources):
            self.assertEqual(
                runner.source_cargo_lock_workspaces(),
                {".", "first-party"},
            )

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

    def test_mutation_aggregate_merges_all_complete_shards(self) -> None:
        manifest = mutation_manifest()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(root, manifest)
            release_manifest = run_mutation_aggregate(root, manifest)
            release = json.loads(release_manifest.read_text(encoding="utf-8"))
            union = json.loads(
                (root / release["mutation_union"]).read_text(encoding="utf-8")
            )
        self.assertEqual(len(release["results"]), 4)
        self.assertEqual(len(union["shards"]), 4)
        self.assertEqual(union["total_mutants"], 0)

    def test_mutation_aggregate_rejects_a_missing_shard(self) -> None:
        manifest = mutation_manifest()
        suites = runner.selected_suites(manifest, [], "mutation", "release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(root, manifest, omitted={suites[-1]["id"]})
            with self.assertRaisesRegex(runner.ValidationError, "missing result"):
                run_mutation_aggregate(root, manifest)

    def test_mutation_aggregate_rejects_a_duplicate_shard(self) -> None:
        manifest = mutation_manifest()
        suites = runner.selected_suites(manifest, [], "mutation", "release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(
                root,
                manifest,
                shard_overrides={suites[1]["id"]: suites[0]["shard"]},
            )
            with self.assertRaisesRegex(
                runner.ValidationError, "duplicate mutation shard identity"
            ):
                run_mutation_aggregate(root, manifest)

    def test_mutation_aggregate_rejects_a_mismatched_shard(self) -> None:
        manifest = mutation_manifest()
        suites = runner.selected_suites(manifest, [], "mutation", "release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(
                root,
                manifest,
                shard_overrides={
                    suites[1]["id"]: {
                        "suite": "other-mutation",
                        "index": 1,
                        "total": 4,
                    }
                },
            )
            with self.assertRaisesRegex(
                runner.ValidationError, "mutation shard identity is"
            ):
                run_mutation_aggregate(root, manifest)

    def test_mutation_aggregate_rejects_a_mismatched_shard_command(self) -> None:
        manifest = mutation_manifest()
        suites = runner.selected_suites(manifest, [], "mutation", "release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(root, manifest)
            result = root / "results" / suites[1]["id"] / "result.json"
            evidence = json.loads(result.read_text(encoding="utf-8"))
            evidence["command"] = suites[0]["command"]
            result.write_text(json.dumps(evidence), encoding="utf-8")
            with self.assertRaisesRegex(
                runner.ValidationError, "mutation shard command is"
            ):
                run_mutation_aggregate(root, manifest)

    def test_mutation_aggregate_rejects_a_sha_mismatch(self) -> None:
        manifest = mutation_manifest()
        suites = runner.selected_suites(manifest, [], "mutation", "release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(
                root,
                manifest,
                commit_overrides={suites[2]["id"]: "b" * 40},
            )
            with self.assertRaisesRegex(runner.ValidationError, "expected"):
                run_mutation_aggregate(root, manifest)

    def test_mutation_aggregate_rejects_union_level_stale_triage(self) -> None:
        manifest = mutation_manifest()
        triage = f"""schema = 1
[[accepted]]
fingerprint = "{"f" * 64}"
reason = "fixture"
reviewer = "fixture"
expires = "2099-01-01"
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_mutation_artifacts(root, manifest)
            with self.assertRaisesRegex(
                runner.ValidationError, "stale mutation triage entries"
            ):
                run_mutation_aggregate(root, manifest, triage)

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

    def test_interop_case_suites_are_portable(self) -> None:
        suites = [
            suite
            for suite in self.manifest["suite"]
            if suite["id"].startswith("interop-")
        ]
        self.assertTrue(suites)
        self.assertTrue(all(suite["platform"] == "any" for suite in suites))

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
