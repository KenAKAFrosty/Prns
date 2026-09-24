from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

from validation.hardening import embedded_miri_execution as execution


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "validation" / "hardening" / "embedded_miri.py"
CORE_SCRIPT = ROOT / "validation" / "hardening" / "miri.sh"
SPEC = importlib.util.spec_from_file_location("embedded_miri", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
embedded_miri = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = embedded_miri
SPEC.loader.exec_module(embedded_miri)


class EmbeddedMiriTests(unittest.TestCase):
    def test_inventory_names_each_required_component_scenario(self) -> None:
        scenarios = embedded_miri.load_inventory()

        self.assertEqual(
            [(scenario.component, scenario.identifier) for scenario in scenarios],
            [
                ("sx126x", "sx126x-state-machine"),
                ("lr1110", "lr1110-state-machine"),
                ("embedded-persistence", "flash-journal-state-machine"),
            ],
        )
        self.assertEqual(len(scenarios[2].quick_filters), 15)

    def test_validation_registry_launches_miri_as_a_repository_module(self) -> None:
        manifest = embedded_miri.tomllib.loads(
            (ROOT / "validation" / "manifest.toml").read_text(encoding="utf-8")
        )
        suites = {
            suite["id"]: suite
            for suite in manifest["suite"]
            if suite["id"] in {"embedded-miri-quick", "embedded-miri-full"}
        }

        for suite in suites.values():
            self.assertEqual(
                suite["command"][:3],
                [
                    "__RUNNER_PYTHON__",
                    "-m",
                    "validation.hardening.embedded_miri",
                ],
            )

    def test_quick_and_full_modes_select_explicit_borrow_models(self) -> None:
        quick = embedded_miri.mode_configuration(embedded_miri.Mode.QUICK)
        full = embedded_miri.mode_configuration(embedded_miri.Mode.FULL)

        self.assertEqual(
            (quick.borrow_models, quick.coverage, quick.runner),
            ((embedded_miri.BorrowModel.STACKED,), "stacked", "miri-stacked"),
        )
        self.assertEqual(
            (full.borrow_models, full.coverage, full.runner),
            (
                (
                    embedded_miri.BorrowModel.STACKED,
                    embedded_miri.BorrowModel.TREE,
                ),
                "stacked-and-tree",
                "miri-stacked-tree",
            ),
        )

    def test_borrow_models_replace_inherited_miri_flags(self) -> None:
        with mock.patch.dict(
            os.environ,
            {"MIRIFLAGS": "-Zmiri-tree-borrows -Zmiri-disable-isolation"},
        ):
            stacked = embedded_miri.miri_environment(
                embedded_miri.BorrowModel.STACKED
            )
            tree = embedded_miri.miri_environment(embedded_miri.BorrowModel.TREE)

        self.assertEqual(stacked["MIRIFLAGS"], "")
        self.assertEqual(tree["MIRIFLAGS"], "-Zmiri-tree-borrows")

    def test_core_runner_replaces_inherited_miri_flags(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            binaries = temporary / "bin"
            binaries.mkdir()
            capture = temporary / "miri-flags"
            self.write_executable(binaries / "python3", "echo nightly-test\n")
            self.write_executable(binaries / "rustup", "exit 0\n")
            self.write_executable(
                binaries / "cargo",
                'if [ "$3" != "setup" ]; then printf "%s\\n" "$MIRIFLAGS" >> "$CAPTURE"; fi\n',
            )
            environment = os.environ.copy()
            environment.update(
                {
                    "CAPTURE": str(capture),
                    "MIRIFLAGS": "-Zmiri-tree-borrows -Zmiri-disable-isolation",
                    "PATH": f"{binaries}:{environment['PATH']}",
                }
            )

            subprocess.run(
                ("/bin/bash", str(CORE_SCRIPT), "--stacked", "one-filter"),
                cwd=ROOT,
                env=environment,
                check=True,
                capture_output=True,
            )
            subprocess.run(
                ("/bin/bash", str(CORE_SCRIPT), "--tree", "one-filter"),
                cwd=ROOT,
                env=environment,
                check=True,
                capture_output=True,
            )

            self.assertEqual(
                capture.read_text(encoding="utf-8").splitlines(),
                [
                    "-Zmiri-env-forward=PROPTEST_CASES "
                    "-Zmiri-env-forward=PROPTEST_DISABLE_FAILURE_PERSISTENCE",
                    "-Zmiri-env-forward=PROPTEST_CASES "
                    "-Zmiri-env-forward=PROPTEST_DISABLE_FAILURE_PERSISTENCE "
                    "-Zmiri-tree-borrows",
                ],
            )

    def test_log_records_exact_model_flags_and_toolchain(self) -> None:
        identity = embedded_miri.ToolchainIdentity(
            channel="nightly-2026-01-02",
            rustc_version="rustc test",
            miri_version="miri test",
        )

        rendered = embedded_miri.render_log(
            embedded_miri.BorrowModel.TREE,
            identity,
            [b"runner output"],
        )

        self.assertEqual(
            rendered,
            b'borrow-model=tree\n'
            b'miri-flags=["-Zmiri-tree-borrows"]\n'
            b'toolchain=nightly-2026-01-02\n'
            b'rustc=rustc test\n'
            b'miri=miri test\n\n'
            b'runner output',
        )

    def test_miri_test_count_comes_from_successful_runner_output(self) -> None:
        output = (
            b"running 16 tests\n"
            b"test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; "
            b"67 filtered out\n"
        )
        self.assertEqual(embedded_miri.parse_completed_tests(output), 16)
        with self.assertRaises(embedded_miri.EmbeddedMiriError):
            embedded_miri.parse_completed_tests(b"test result: FAILED")

    def test_focused_filters_cannot_pass_with_missing_or_ignored_tests(self) -> None:
        for output in (
            b"test result: ok. 0 passed; 0 failed; 0 ignored;",
            b"test result: ok. 1 passed; 0 failed; 1 ignored;",
        ):
            with self.subTest(output=output), self.assertRaises(embedded_miri.EmbeddedMiriError):
                embedded_miri.parse_completed_tests(output)

    def test_release_requires_bounded_focused_miri_and_schedules_exhaustive_miri(self) -> None:
        manifest = embedded_miri.tomllib.loads((ROOT / "validation/manifest.toml").read_text())
        suites = {suite["id"]: suite for suite in manifest["suite"]}
        self.assertEqual(suites["embedded-miri-quick"]["timeout_seconds"], 900)
        self.assertEqual(suites["embedded-miri-quick"]["tiers"], ["pr", "release"])
        self.assertEqual(suites["embedded-miri-full"]["tiers"], ["scheduled"])
        self.assertEqual(embedded_miri.mode_configuration(embedded_miri.Mode.QUICK).scope, "focused")
        self.assertEqual(embedded_miri.mode_configuration(embedded_miri.Mode.FULL).scope, "exhaustive")

    def test_runner_clears_only_owned_component_evidence(self) -> None:
        scenarios = embedded_miri.load_inventory()
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            owned = artifacts / "sx126x.assurance.json"
            log = artifacts / "sx126x-stacked.log"
            unrelated = artifacts / "result.json"
            for path in (owned, log, unrelated):
                path.write_text("evidence", encoding="utf-8")

            embedded_miri.clear_owned_artifacts(scenarios, artifacts)

            self.assertFalse(owned.exists())
            self.assertFalse(log.exists())
            self.assertTrue(unrelated.exists())

    def test_artifact_directory_is_validated_before_creation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            root = temporary / "artifacts"
            outside = temporary / "outside"
            with mock.patch.dict(
                os.environ,
                {
                    "PRNS_VALIDATION_ARTIFACT_ROOT": str(root),
                    "PRNS_VALIDATION_ARTIFACT_DIR": str(outside),
                },
            ):
                with self.assertRaises(embedded_miri.EmbeddedMiriError):
                    embedded_miri.artifact_directory()
            self.assertFalse(outside.exists())

    def test_duplicate_component_inventory_is_rejected(self) -> None:
        scenario = """
schema = 1
[[scenario]]
component = "sx126x"
id = "sx126x-state-machine"
manifest = "prns-interfaces/impls/embassy/Cargo.toml"
features = ["lora"]
quick_filters = ["one"]
full_filters = ["one"]
sources = ["prns-interfaces/impls/embassy/src/radios/sx126x.rs"]
[[scenario]]
component = "sx126x"
id = "another-scenario"
manifest = "prns-interfaces/impls/embassy/Cargo.toml"
features = ["lora"]
quick_filters = ["two"]
full_filters = ["two"]
sources = ["prns-interfaces/impls/embassy/src/radios/sx126x.rs"]
"""
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "inventory.toml"
            inventory.write_text(scenario, encoding="utf-8")
            with self.assertRaises(embedded_miri.EmbeddedMiriError):
                embedded_miri.load_inventory(inventory)

    def test_failed_miri_preparation_points_to_the_readiness_doctor(self) -> None:
        with mock.patch.object(
            embedded_miri.subprocess,
            "run",
            side_effect=OSError("rustup missing"),
        ):
            with self.assertRaisesRegex(
                embedded_miri.EmbeddedMiriError, "doctor embedded-assurance"
            ):
                embedded_miri.prepare_miri("nightly-test")

    def test_pre_push_policy_refuses_to_install_a_missing_toolchain(self) -> None:
        unavailable = subprocess.CompletedProcess(
            args=("rustup",),
            returncode=1,
        )
        with mock.patch.object(
            embedded_miri.subprocess,
            "run",
            return_value=unavailable,
        ) as run:
            with self.assertRaisesRegex(
                embedded_miri.EmbeddedMiriError, "doctor embedded-assurance"
            ):
                embedded_miri.prepare_miri(
                    "nightly-test",
                    embedded_miri.ProvisioningPolicy.REQUIRE_EXISTING,
                )

        run.assert_called_once_with(
            ("rustup", "run", "nightly-test", "rustc", "--version"),
            cwd=ROOT,
            capture_output=True,
        )

    def test_pre_push_policy_refuses_to_add_missing_components(self) -> None:
        available = subprocess.CompletedProcess(
            args=("rustup",),
            returncode=0,
        )
        installed = subprocess.CompletedProcess(
            args=("rustup",),
            returncode=0,
            stdout="rustc-test\n",
        )
        with mock.patch.object(
            embedded_miri.subprocess,
            "run",
            side_effect=(available, installed),
        ) as run:
            with self.assertRaisesRegex(
                embedded_miri.EmbeddedMiriError, "doctor embedded-assurance"
            ):
                embedded_miri.prepare_miri(
                    "nightly-test",
                    embedded_miri.ProvisioningPolicy.REQUIRE_EXISTING,
                )

        self.assertEqual(run.call_count, 2)

    def test_provisioning_policy_rejects_unknown_values(self) -> None:
        with self.assertRaisesRegex(
            embedded_miri.EmbeddedMiriError,
            embedded_miri.PROVISIONING_ENV,
        ):
            embedded_miri.provisioning_policy(
                {embedded_miri.PROVISIONING_ENV: "sometimes"}
            )

    def test_miri_rejection_is_structural_failure_with_preserved_log(self) -> None:
        scenario = replace(
            embedded_miri.load_inventory()[0],
            quick_filters=("rejected-filter",),
        )
        identity = embedded_miri.ToolchainIdentity(
            channel="nightly-test",
            rustc_version="rustc test",
            miri_version="miri test",
        )
        result = subprocess.CompletedProcess(
            args=("cargo",),
            returncode=1,
            stdout=b"test result: FAILED",
            stderr=b"Miri rejected an operation",
        )

        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            with mock.patch.object(embedded_miri.subprocess, "run", return_value=result):
                with self.assertRaises(
                    embedded_miri.ProofExecutionError
                ) as raised:
                    embedded_miri.run_scenario(
                        scenario,
                        embedded_miri.Mode.QUICK,
                        embedded_miri.BorrowModel.STACKED,
                        identity,
                        artifacts,
                    )

            self.assertEqual(
                raised.exception.failure.kind,
                embedded_miri.FailureKind.STRUCTURAL_VIOLATION,
            )
            log = artifacts / "sx126x-stacked.log"
            self.assertTrue(log.is_file())
            self.assertIn(b"Miri rejected an operation", log.read_bytes())

    def test_recorder_failure_does_not_mask_the_miri_rejection(self) -> None:
        scenario = embedded_miri.load_inventory()[0]
        original = embedded_miri.ProofFailure(
            kind=embedded_miri.FailureKind.STRUCTURAL_VIOLATION,
            diagnostic="original Miri rejection",
        )

        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            with (
                mock.patch.object(
                    embedded_miri,
                    "load_inventory",
                    return_value=(scenario,),
                ),
                mock.patch.object(
                    embedded_miri,
                    "artifact_directory",
                    return_value=artifacts,
                ),
                mock.patch.object(embedded_miri, "prepare_miri"),
                mock.patch.object(
                    embedded_miri,
                    "nightly_toolchain",
                    return_value="nightly-test",
                ),
                mock.patch.object(
                    embedded_miri,
                    "tool_version",
                    side_effect=("rustc test", "miri test"),
                ),
                mock.patch.object(
                    embedded_miri,
                    "run_scenario",
                    side_effect=embedded_miri.ProofExecutionError(original),
                ),
                mock.patch.object(
                    embedded_miri,
                    "record_failure",
                    side_effect=subprocess.CalledProcessError(1, ("recorder",)),
                ),
            ):
                with self.assertRaises(embedded_miri.EmbeddedMiriError) as raised:
                    embedded_miri.run(embedded_miri.Mode.QUICK)

        diagnostic = str(raised.exception)
        self.assertIn("original Miri rejection", diagnostic)
        self.assertIn("failure evidence could not be recorded", diagnostic)

    def test_full_discovery_unions_overlapping_filters_without_losing_tests(self) -> None:
        outputs = [
            subprocess.CompletedProcess([], 0, b"z: test\ny: test\n", b""),
            subprocess.CompletedProcess([], 0, b"y: test\na: test\n", b""),
        ]
        with mock.patch.object(execution.subprocess, "run", side_effect=outputs) as run:
            names = execution.discover_tests(("cargo", "miri", "test"), ("one", "two"), ROOT, {})
        self.assertEqual(names, ("y", "z", "a"))
        self.assertEqual(run.call_count, 2)
        with mock.patch.object(
            execution.subprocess, "run",
            return_value=subprocess.CompletedProcess([], 0, b"", b""),
        ):
            with self.assertRaises(execution.MiriExecutionError):
                execution.discover_tests(("cargo",), ("empty",), ROOT, {})

    def test_parallel_execution_keeps_every_result_and_failed_test_log(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "runner.py"
            script.write_text(
                "import sys\n"
                "name = sys.argv[sys.argv.index('--') + 1]\n"
                "print(name, flush=True)\n"
                "sys.exit(1 if name == 'bad' else 0)\n"
            )
            results = list(execution.run_tests(
                (sys.executable, str(script)), ("good", "bad", "also-good"), root,
                os.environ.copy(), root / "logs",
            ))
            self.assertEqual(
                {item.name: item.returncode for item in results},
                {"good": 0, "bad": 1, "also-good": 0},
            )
            self.assertEqual(len(list((root / "logs").glob("*.log"))), 3)
            self.assertTrue(all(item.name.encode() in item.output for item in results))
            for names in [(), ("duplicate", "duplicate")]:
                with self.assertRaises(execution.MiriExecutionError):
                    list(execution.run_tests((sys.executable,), names, root, {}, root / "invalid"))
            for workers in (0, 5, True, "2"):
                with self.assertRaises(execution.MiriExecutionError):
                    list(execution.run_tests(
                        (sys.executable,), ("case",), root, {}, root / "invalid", workers=workers,
                    ))

    def test_full_shard_executes_only_its_assignment_but_retains_full_inventory(self) -> None:
        from validation.hardening.embedded_miri_shards import Shard
        from validation.tests.test_embedded_miri_shards import output_for

        scenario = embedded_miri.load_inventory()[0]
        identity = embedded_miri.ToolchainIdentity("nightly-test", "rustc test", "miri test")
        inventory = ("one", "two", "three", "four")
        results = tuple(execution.TestExecution(name, 0, output_for(name)) for name in ("four", "two"))
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(embedded_miri, "discover_tests", return_value=inventory),
            mock.patch.object(embedded_miri, "run_tests", return_value=iter(results)) as run,
        ):
            observed = embedded_miri.execute_full_scenario(
                scenario, embedded_miri.BorrowModel.TREE, identity, Path(directory),
                Shard(1, 2), workers=2,
            )
            self.assertEqual(observed.inventory, inventory)
            self.assertEqual(observed.results, results)
            self.assertEqual(observed.observation.completed_tests, 2)
            self.assertEqual(run.call_args.args[1], ("two", "four"))
            self.assertEqual(run.call_args.kwargs, {"workers": 2})
            self.assertIn(b'assigned-tests=["two", "four"]', observed.observation.log.read_bytes())

    def test_full_evidence_requires_each_discovered_test_to_execute(self) -> None:
        scenario = embedded_miri.load_inventory()[0]
        identity = embedded_miri.ToolchainIdentity("nightly-test", "rustc test", "miri test")
        for output in [
            b"test result: ok. 0 passed; 0 failed; 1 ignored;",
            b"test result: ok. 2 passed; 0 failed; 0 ignored;",
        ]:
            with (
                tempfile.TemporaryDirectory() as directory,
                mock.patch.object(embedded_miri, "discover_tests", return_value=("case",)),
                mock.patch.object(
                    embedded_miri, "run_tests",
                    return_value=iter([execution.TestExecution("case", 0, output)]),
                ),
            ):
                with self.assertRaises(embedded_miri.EmbeddedMiriError):
                    embedded_miri.run_full_scenario(
                        scenario, embedded_miri.BorrowModel.STACKED, identity, Path(directory),
                    )

    def test_full_failure_preserves_successful_and_failed_cases(self) -> None:
        scenario = embedded_miri.load_inventory()[0]
        identity = embedded_miri.ToolchainIdentity("nightly-test", "rustc test", "miri test")
        results = [
            execution.TestExecution(
                "good", 0, b"test good ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;",
            ),
            execution.TestExecution("bad", 1, b"Miri rejected the bad case"),
        ]
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(embedded_miri, "discover_tests", return_value=("good", "bad")),
            mock.patch.object(embedded_miri, "run_tests", return_value=iter(results)),
        ):
            with self.assertRaises(embedded_miri.ProofExecutionError):
                embedded_miri.run_full_scenario(
                    scenario, embedded_miri.BorrowModel.TREE, identity, Path(directory),
                )
            log = (Path(directory) / "sx126x-tree.log").read_bytes()
            self.assertIn(results[0].output, log)
            self.assertIn(results[1].output, log)

    def test_full_inventory_rejects_duplicate_or_missing_results(self) -> None:
        scenario = embedded_miri.load_inventory()[0]
        identity = embedded_miri.ToolchainIdentity("nightly-test", "rustc test", "miri test")
        passed = b"test result: ok. 1 passed; 0 failed; 0 ignored;"
        for names in [("one",), ("one", "one"), ("one", "unknown")]:
            with (
                tempfile.TemporaryDirectory() as directory,
                mock.patch.object(embedded_miri, "discover_tests", return_value=("one", "two")),
                mock.patch.object(
                    embedded_miri, "run_tests",
                    return_value=iter(
                        execution.TestExecution(name, 0, f"test {name} ... ok\n".encode() + passed)
                        for name in names
                    ),
                ),
            ):
                with self.assertRaises(embedded_miri.EmbeddedMiriError):
                    embedded_miri.run_full_scenario(
                        scenario, embedded_miri.BorrowModel.STACKED, identity, Path(directory),
                    )

    @staticmethod
    def write_executable(path: Path, body: str) -> None:
        path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
        path.chmod(0o755)

if __name__ == "__main__":
    unittest.main()
