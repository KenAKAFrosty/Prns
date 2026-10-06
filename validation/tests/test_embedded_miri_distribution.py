from __future__ import annotations

import copy
import json
import tempfile
import unittest
from contextlib import ExitStack
from dataclasses import asdict
from pathlib import Path
from unittest import mock

from validation import run as control
from validation.hardening import embedded_miri as miri
from validation.hardening import embedded_miri_distribution as distribution
from validation.hardening.embedded_miri_execution import TestExecution
from validation.hardening.embedded_miri_shards import Shard, ShardError, write_fragment
from validation.tests.test_embedded_miri_shards import output_for


COMMIT = "a" * 40
NAMES = tuple(f"fixture::case_{index}" for index in range(8))


class EmbeddedMiriDistributionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.scenarios = miri.load_inventory()
        self.manifest = control.load_manifest()
        self.suites = control.selected_suites(
            self.manifest, [control.EMBEDDED_MIRI_FULL], None, None,
        )
        self.identity = miri.ToolchainIdentity(miri.nightly_toolchain(), "rustc fixture", "miri fixture")
        self.environment = {
            "toolchain": asdict(self.identity),
            "host": {"platform": "linux", "machine": "x86_64"},
            "build_flags": dict.fromkeys(distribution.BUILD_FLAGS),
        }
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.clean = self.stack.enter_context(mock.patch.object(distribution, "clean_commit", return_value=COMMIT))
        self.record = self.stack.enter_context(mock.patch.object(miri, "record_proof"))

    def directory(self, index: int) -> Path:
        return self.root / "results" / self.suites[index]["id"]

    def write_fixture(self, inventories: dict[str, tuple[str, ...]] | None = None) -> None:
        inventories = inventories or {}
        for index, suite in enumerate(self.suites):
            directory = self.directory(index)
            directory.mkdir(parents=True)
            result = {
                "schema": control.EVIDENCE_SCHEMA, "suite": suite["id"],
                "domain": suite["domain"], "commit": COMMIT,
                "platform": "linux", "required_platform": suite["platform"],
                "worktree_clean": True,
                "command": control.command_for(suite, 0, directory),
                "tool_versions": {"rustc-nightly": self.identity.rustc_version},
                "started_at": "2026-10-05T00:00:00Z", "finished_at": "2026-10-05T00:01:00Z",
                "duration_seconds": 60, "status": "passed", "exit_code": 0,
                "timed_out": False, "spawn_error": None, "shard": suite["shard"],
            }
            (directory / "result.json").write_text(json.dumps(result))
            for scenario in self.scenarios:
                for model in miri.mode_configuration(miri.Mode.FULL).borrow_models:
                    names = inventories.get(model.value, NAMES)
                    shard = Shard(index, len(self.suites))
                    logs = directory / f"{scenario.component}-{model.value}-tests"
                    logs.mkdir()
                    results = []
                    for position, name in enumerate(shard.select(names)):
                        output = output_for(name)
                        (logs / f"{position:04}.log").write_bytes(output)
                        results.append(TestExecution(name, 0, output))
                    write_fragment(
                        directory / f"{scenario.component}-{model.value}.shard.json",
                        {**distribution.constant_context(scenario, model, COMMIT), **self.environment},
                        shard, names, tuple(results),
                    )

    def test_complete_collection_records_all_components_and_both_models(self) -> None:
        self.write_fixture()
        distribution.collect_proofs(list(reversed(self.suites)), self.root, COMMIT)
        self.assertEqual(self.record.call_count, len(self.scenarios))
        for call, scenario in zip(self.record.call_args_list, self.scenarios):
            observed_scenario, configuration, observations, output, identity = call.args
            self.assertEqual((observed_scenario, configuration.coverage, identity), (scenario, "stacked-and-tree", self.identity))
            self.assertEqual(tuple(observation.completed_tests for observation in observations), (8, 8))
            self.assertEqual(output, self.root / "results" / control.EMBEDDED_MIRI_FULL)
            for observation in observations:
                log = observation.log.read_bytes()
                self.assertIn(b"validation-shards=", log)
                self.assertIn(b"miri-shards=", log)
                for name in NAMES:
                    self.assertIn(output_for(name), log)
        self.assertEqual(self.clean.call_args_list, [mock.call(COMMIT), mock.call(COMMIT)])

    def test_invalid_validation_results_cannot_produce_any_proof(self) -> None:
        self.write_fixture()
        path = self.directory(3) / "result.json"
        original = json.loads(path.read_text())
        mutations = {
            "failed": {"status": "failed", "exit_code": 1},
            "timeout": {"timed_out": True},
            "dirty": {"worktree_clean": False},
            "stale": {"commit": "b" * 40},
            "wrong suite": {"suite": self.suites[0]["id"]},
            "wrong command": {"command": ["python", "wrong"]},
            "wrong platform": {"required_platform": "windows"},
            "wrong host": {"platform": "macos"},
            "wrong shard": {"shard": self.suites[0]["shard"]},
            "tool unavailable": {"tool_versions": {"rustc": "unavailable: missing"}},
            "different compiler": {"tool_versions": {"rustc-nightly": "different compiler"}},
        }
        for label, change in mutations.items():
            with self.subTest(label=label):
                path.write_text(json.dumps({**original, **change}))
                with self.assertRaises(ShardError):
                    distribution.collect_proofs(self.suites, self.root, COMMIT)
                self.record.assert_not_called()
        path.unlink()
        with self.assertRaises(ShardError):
            distribution.collect_proofs(self.suites, self.root, COMMIT)
        self.record.assert_not_called()

    def test_last_component_failure_prevents_earlier_component_proofs(self) -> None:
        self.write_fixture()
        path = self.directory(3) / "embedded-persistence-tree-tests/0000.log"
        path.write_bytes(b"truncated")
        with self.assertRaisesRegex(ShardError, "fingerprint differs"):
            distribution.collect_proofs(self.suites, self.root, COMMIT)
        self.record.assert_not_called()

    def test_collect_rejects_missing_and_duplicate_shards(self) -> None:
        self.write_fixture()
        for suites in (self.suites[:-1], [*self.suites[:-1], self.suites[0]]):
            with self.subTest(suites=suites), self.assertRaises(ShardError):
                distribution.collect_proofs(suites, self.root, COMMIT)
        self.record.assert_not_called()

    def test_context_cannot_mix_toolchains_flags_or_hosts_across_models(self) -> None:
        self.write_fixture()
        paths = [self.directory(index) / "embedded-persistence-tree.shard.json" for index in range(8)]
        originals = [json.loads(path.read_text()) for path in paths]
        changes = {
            "compiler": lambda value: value["toolchain"].update(rustc_version="different compiler"),
            "nightly": lambda value: value["toolchain"].update(channel="unpinned"),
            "host": lambda value: value["host"].update(machine="aarch64"),
            "flags": lambda value: value["build_flags"].update(RUSTFLAGS="--cfg different"),
            "invalid flag": lambda value: value["build_flags"].update(RUSTFLAGS=3),
            "extra": lambda value: value.update(unexpected=True),
        }
        for label, change in changes.items():
            with self.subTest(label=label):
                for path, original in zip(paths, originals):
                    value = copy.deepcopy(original)
                    change(value["context"])
                    path.write_text(json.dumps(value))
                with self.assertRaises(ShardError):
                    distribution.collect_proofs(self.suites, self.root, COMMIT)
                self.record.assert_not_called()

    def test_borrow_models_must_discover_the_same_inventory(self) -> None:
        self.write_fixture({"tree": tuple(reversed(NAMES))})
        with self.assertRaisesRegex(ShardError, "different full inventories"):
            distribution.collect_proofs(self.suites, self.root, COMMIT)
        self.record.assert_not_called()

    def test_source_change_during_collection_refuses_proofs(self) -> None:
        self.write_fixture()
        self.clean.side_effect = [COMMIT, ShardError("source changed")]
        with self.assertRaisesRegex(ShardError, "source changed"):
            distribution.collect_proofs(self.suites, self.root, COMMIT)
        self.record.assert_not_called()

    def test_individual_execution_writes_only_partial_records(self) -> None:
        artifacts = self.directory(0)
        artifacts.mkdir(parents=True)
        shard = Shard(0, 8)
        result = TestExecution(NAMES[0], 0, output_for(NAMES[0]))

        def execute(scenario, model, identity, directory, assigned, *, workers):
            self.assertEqual((identity, directory, assigned, workers), (self.identity, artifacts, shard, 2))
            return miri.ModelExecution(
                miri.Observation(model, 1, directory / f"{scenario.component}-{model.value}.log"),
                NAMES, (result,),
            )

        with (
            mock.patch.object(miri, "artifact_directory", return_value=artifacts),
            mock.patch.object(miri, "prepare_identity", return_value=self.identity),
            mock.patch.object(miri, "execute_full_scenario", side_effect=execute),
        ):
            distribution.run_shard(shard)
        self.assertEqual(len(list(artifacts.glob("*.shard.json"))), 6)
        self.record.assert_not_called()
        self.assertEqual(list(artifacts.glob("*.assurance.json")), [])

    def test_local_concurrency_is_bounded_by_cpu_and_worker_budgets(self) -> None:
        for processors, expected in ((None, 1), (1, 1), (4, 1), (8, 2), (128, 2)):
            with mock.patch.object(distribution.os, "cpu_count", return_value=processors):
                self.assertEqual(distribution.local_parallelism(), expected)

    def test_shard_collection_requires_the_whole_registered_family(self) -> None:
        with mock.patch.object(distribution, "collect_proofs") as collect:
            control.collect_embedded_miri_shards(self.manifest, self.suites[:1], COMMIT, require_complete=False)
            collect.assert_not_called()
            with self.assertRaisesRegex(control.ValidationError, "every registered shard"):
                control.collect_embedded_miri_shards(self.manifest, self.suites[:1], COMMIT, require_complete=True)
            with mock.patch.object(control, "validation_artifact_root", return_value=self.root):
                control.collect_embedded_miri_shards(self.manifest, self.suites, COMMIT, require_complete=True)
            collect.assert_called_once_with(self.suites, self.root, COMMIT)

    def test_parent_suite_selection_expands_and_deduplicates_explicit_shards(self) -> None:
        selected = control.selected_suites(
            self.manifest, [control.EMBEDDED_MIRI_FULL, self.suites[0]["id"]], None, None,
        )
        self.assertEqual(selected, self.suites)
        self.assertEqual(len(selected), 8)


if __name__ == "__main__":
    unittest.main()
