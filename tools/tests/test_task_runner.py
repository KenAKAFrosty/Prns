from __future__ import annotations

import copy
from importlib.machinery import SourceFileLoader
import importlib.util
from pathlib import Path
import re
import subprocess
import sys
import unittest
from unittest import mock

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
RUNNER_PATH = ROOT / "tools" / "prns"
LOADER = SourceFileLoader("prns_task_runner", str(RUNNER_PATH))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
assert SPEC is not None
runner = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(runner)


class TaskRegistryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = runner.load_manifest()

    def test_live_registry_is_complete(self) -> None:
        self.assertEqual(runner.validate_manifest(self.manifest), [])

    def test_duplicate_task_ids_are_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["task"].append(copy.deepcopy(manifest["task"][0]))
        with self.assertRaisesRegex(runner.ToolingError, "duplicate task id"):
            runner.task_map(manifest)

    def test_unclassified_implementation_is_rejected(self) -> None:
        orphan = ROOT / "tools" / "release" / "task_runner_orphan.py"
        orphan.write_text("# temporary task registry self-test\n", encoding="utf-8")
        try:
            errors = runner.validate_manifest(self.manifest, check_callers=False)
            self.assertTrue(any("unclassified tool implementations" in error for error in errors))
        finally:
            orphan.unlink()

    def test_python_cache_artifacts_are_not_tool_implementations(self) -> None:
        cache = ROOT / "tools" / "release" / "__pycache__"
        cache.mkdir(exist_ok=True)
        artifact = cache / "task_runner_cache_test.cpython-999.pyc"
        artifact.write_bytes(b"temporary interpreter cache")
        try:
            self.assertNotIn(
                artifact.relative_to(ROOT).as_posix(),
                runner.implementation_inventory(),
            )
        finally:
            artifact.unlink()

    def test_scattered_component_script_is_rejected(self) -> None:
        directory = ROOT / ".task-runner-test-component" / "scripts"
        directory.mkdir(parents=True)
        orphan = directory / "orphan.sh"
        orphan.write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        try:
            errors = runner.validate_manifest(self.manifest, check_callers=False)
            self.assertTrue(
                any("scattered component directories" in error for error in errors)
            )
        finally:
            orphan.unlink()
            directory.rmdir()
            directory.parent.rmdir()

    def test_hierarchical_task_names_resolve(self) -> None:
        tasks = runner.task_map(self.manifest)
        task, arguments = runner.resolve_hierarchical(
            tasks,
            ["release", "candidate", "extract", "--", "bundle.tar.gz", "target/candidate"],
        )
        self.assertEqual(task["id"], "release.candidate.extract")
        self.assertEqual(arguments, ["bundle.tar.gz", "target/candidate"])

    def test_every_public_task_has_visible_safety_metadata(self) -> None:
        for task in runner.task_map(self.manifest).values():
            self.assertIn(task["effect"], runner.VALID_EFFECTS)
            self.assertTrue(task["summary"].strip())
            self.assertTrue(task["platforms"])
            self.assertTrue(task["audience"])

    def test_clone_front_door_and_crate_readme_links_stay_local_and_live(self) -> None:
        for relative in ("README.md", "prnsd/README.md", "personal-rns/README.md"):
            path = ROOT / relative
            source = path.read_text(encoding="utf-8")
            for target in re.findall(r"\]\(([^)]+)\)", source):
                if (
                    target.startswith(("#", "/", "mailto:"))
                    or "://" in target
                ):
                    continue
                local = target.split("#", maxsplit=1)[0]
                self.assertTrue(
                    (path.parent / local).exists(),
                    f"{relative} has a dead local link: {target}",
                )
            self.assertNotIn("cargo add prnsd", source)

    def test_task_implementation_cannot_cross_domain_boundaries(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        task = next(task for task in manifest["task"] if task["id"] == "build.android")
        task["entrypoint"] = ["bash", "tools/device/techo-flash.sh"]
        errors = runner.validate_manifest(manifest, check_callers=False)
        self.assertTrue(
            any("implementation must live in its domain directory" in error for error in errors)
        )

    def test_wasm_bindgen_setup_matches_lockfile_and_release_workflows(self) -> None:
        task = runner.task_map(self.manifest)["build.wasm-docs.stage"]
        lock = tomllib.loads((ROOT / "prns-wasm" / "Cargo.lock").read_text(encoding="utf-8"))
        version = next(
            package["version"]
            for package in lock["package"]
            if package["name"] == "wasm-bindgen"
        )
        setup = task["setup"]
        self.assertIn(f"--version {version} --locked", setup)
        for workflow_name in ("release-readiness.yml", "flasher-candidate.yml"):
            workflow = (
                ROOT / ".github" / "workflows" / workflow_name
            ).read_text(encoding="utf-8")
            self.assertIn(setup, workflow)
        candidate = (
            ROOT / ".github" / "workflows" / "flasher-candidate.yml"
        ).read_text(encoding="utf-8")
        version_pattern = re.escape(version)
        self.assertIn(
            f"wasm-bindgen --version | grep -E '^wasm-bindgen {version_pattern}$'",
            candidate,
        )

    def test_retired_script_callers_are_rejected(self) -> None:
        self.assertIsNotNone(runner.LEGACY_CALL_PATTERN.search("run: bash scripts/legacy.sh"))
        self.assertIsNone(
            runner.LEGACY_CALL_PATTERN.search("run: bash component/scripts/verify.sh")
        )

    def test_doctor_ignores_requirements_for_other_platforms(self) -> None:
        task = copy.deepcopy(self.manifest["task"][0])
        task["platforms"] = ["windows" if runner.native_platform() != "windows" else "linux"]
        task["requires"] = ["definitely-not-a-real-prns-command"]
        self.assertTrue(runner.doctor([task]))

    def test_python_entrypoint_reuses_the_control_plane_interpreter(self) -> None:
        task = {
            "id": "release.test",
            "summary": "exercise interpreter selection",
            "effect": "read-only",
            "platforms": ["any"],
            "entrypoint": ["python", "tools/release/check-host-sdk-versions.py"],
        }
        completed = subprocess.CompletedProcess([], 0)
        with mock.patch.object(runner.subprocess, "run", return_value=completed) as run:
            self.assertEqual(runner.run_task(task, []), 0)
        self.assertEqual(run.call_args.args[0][0], sys.executable)

    def test_task_interruption_returns_standard_exit_status(self) -> None:
        task = {
            "id": "release.test",
            "summary": "exercise interruption handling",
            "effect": "read-only",
            "platforms": ["any"],
            "entrypoint": ["true"],
        }
        with mock.patch.object(runner.subprocess, "run", side_effect=KeyboardInterrupt):
            self.assertEqual(runner.run_task(task, []), 130)

    def test_doctor_profiles_cover_each_beginner_outcome(self) -> None:
        profiles = runner.doctor_profile_map(self.manifest)
        self.assertEqual(
            set(profiles),
            {"getting-started", "node", "rust", "docs", "tests", "benchmarks"},
        )
        self.assertEqual(profiles["docs"]["exact_versions"]["dx"], "0.7.5")
        self.assertEqual(profiles["rust"]["minimum_versions"]["rustc"], "1.90")

    def test_doctor_profile_name_cannot_collide_with_a_task_or_domain(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["doctor_profile"][0]["id"] = "release"
        errors = runner.validate_manifest(manifest, check_callers=False)
        self.assertTrue(any("collides with a task ID or domain" in error for error in errors))

    def test_doctor_profile_rejects_old_versions(self) -> None:
        profile = copy.deepcopy(runner.doctor_profile_map(self.manifest)["rust"])
        with (
            mock.patch.object(runner, "command_path", return_value="/test/tool"),
            mock.patch.object(runner, "command_version", return_value=(1, 0, 0)),
        ):
            self.assertFalse(runner.doctor_profile(profile))

    def test_benchmark_doctor_selects_only_the_host_compiler(self) -> None:
        profile = copy.deepcopy(runner.doctor_profile_map(self.manifest)["benchmarks"])
        commands = []

        def record(command: str) -> str:
            commands.append(command)
            return f"/test/{command}"

        with (
            mock.patch.object(runner, "native_platform", return_value="macos"),
            mock.patch.object(runner, "command_path", side_effect=record),
            mock.patch.object(runner, "command_version", return_value=(99, 0, 0)),
        ):
            self.assertTrue(runner.doctor_profile(profile))
        self.assertIn("cc", commands)
        self.assertNotIn("cl", commands)

    def test_verify_output_explains_guarantees(self) -> None:
        result = subprocess.run(
            [str(RUNNER_PATH), "verify"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertIn("Registry:", result.stdout)
        self.assertIn("Safety:", result.stdout)
        self.assertIn("Ownership:", result.stdout)
        self.assertIn("TOOLING_REGISTRY_OK", result.stdout)

    def test_cargo_tools_is_a_thin_task_runner_alias(self) -> None:
        cargo_config = tomllib.loads(
            (ROOT / ".cargo" / "config.toml").read_text(encoding="utf-8")
        )
        self.assertEqual(
            cargo_config["alias"]["tools"],
            "run --quiet -p prns-tools-command --",
        )
        result = subprocess.run(
            ["cargo", "tools", "explain", "release.source.package"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertIn("Task: release.source.package", result.stdout)
        self.assertIn("Effect: workspace-write", result.stdout)
        passthrough = subprocess.run(
            ["cargo", "tools", "release", "source", "package", "--", "--help"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertIn("--output OUTPUT", passthrough.stdout)


if __name__ == "__main__":
    unittest.main()
