import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("group_scale", ROOT / "tools/build/group-scale.py")
scale = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scale)


class GroupScaleTests(unittest.TestCase):
    def test_profile_keeps_assertions_and_overflow_checks(self):
        env = scale.environment({"CARGO_TARGET_DIR": "/selected/cache", "CARGO_PROFILE_TEST_DEBUG_ASSERTIONS": "false", "LC_ALL": "fr_FR.UTF-8"})
        self.assertEqual(env["CARGO_TARGET_DIR"], "/selected/cache")
        self.assertEqual(env["LC_ALL"], "C")
        self.assertEqual(env["CARGO_PROFILE_TEST_OPT_LEVEL"], "2")
        self.assertEqual(env["CARGO_PROFILE_TEST_DEBUG_ASSERTIONS"], "true")
        self.assertEqual(env["CARGO_PROFILE_TEST_OVERFLOW_CHECKS"], "true")

    def test_cargo_configuration_owns_the_guarded_target_filesystem(self):
        env = {"CARGO_TARGET_DIR": "relative-cache"}
        with patch.object(scale.subprocess, "check_output", return_value='{"target_directory":"/configured/actual-cache"}') as call:
            self.assertEqual(scale.target_directory(["cargo", "+1.96.0"], env, True), Path("/configured/actual-cache"))
            args, kwargs = call.call_args
            self.assertEqual(args[0][:3], ["cargo", "+1.96.0", "metadata"])
            self.assertIn("--no-deps", args[0])
            self.assertIn("--offline", args[0])
            self.assertEqual(kwargs["cwd"], ROOT)
            self.assertEqual(kwargs["env"], env)
        with patch.object(scale.subprocess, "check_output", return_value='{"target_directory":"relative"}'):
            with self.assertRaises(ValueError):
                scale.target_directory(["cargo"], {}, False)

    def test_natural_child_exit_winning_stop_is_not_a_cleanup_failure(self):
        from unittest.mock import Mock
        process = Mock(pid=999)
        process.poll.side_effect = [None, 0]
        with patch.object(scale.os, "killpg", side_effect=ProcessLookupError):
            scale.stop(process)
        process.wait.assert_not_called()

    def test_rss_units_are_platform_specific_and_missing_is_not_zero(self):
        self.assertEqual(scale.peak_rss("  1234 maximum resident set size\n", "darwin"), 1234)
        self.assertEqual(scale.peak_rss("Maximum resident set size (kbytes): 1234\n", "linux"), 1234 * 1024)
        self.assertIsNone(scale.peak_rss("no result", "darwin"))

    def test_only_exact_ignored_test_runs(self):
        for platform, option in [("darwin", "-l"), ("linux", "-v")]:
            command = scale.timing_command(platform, Path("/binary"), "chosen::test")
            self.assertEqual(command[:3], ["/usr/bin/time", option, "/binary"])
            self.assertEqual(command[3:], ["--ignored", "--exact", "chosen::test", "--nocapture", "--test-threads=1"])
        with self.assertRaises(ValueError):
            scale.timing_command("win32", Path("/binary"), "chosen::test")

    def test_emitted_artifact_is_exact_and_unambiguous(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "test"
            binary.touch()
            message = {"reason": "compiler-artifact", "executable": str(binary), "target": {"name": "catalog_capacity", "kind": ["test"]}, "profile": {"test": True}}
            self.assertEqual(scale.artifact([message], "catalog_capacity"), binary.resolve())
            with self.assertRaises(ValueError):
                scale.artifact([message], "group_only_onboarding")
            other = Path(temporary) / "other"
            other.touch()
            with self.assertRaises(ValueError):
                scale.artifact([message, {**message, "executable": str(other)}], "catalog_capacity")

    def test_success_requires_one_owning_measurement(self):
        self.assertEqual(scale.measured_result('test label ... result {"count":1000}', "result "), {"count": 1000})
        for text in ["", 'result {}\nresult {}', "result []"]:
            with self.assertRaises(ValueError):
                scale.measured_result(text, "result ")

    def test_low_disk_never_launches_process(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(scale.shutil, "disk_usage") as usage, patch.object(scale.subprocess, "Popen") as spawn:
            usage.return_value.free = 1
            with self.assertRaisesRegex(ValueError, "Disk floor"):
                scale.guarded(["unused"], cwd=Path(temporary), env={}, stdout=Path(temporary) / "out", stderr=None, floor=2, disk=Path(temporary))
            spawn.assert_not_called()

    @unittest.skipUnless(os.name == "posix", "owning process-group cleanup is Unix-only")
    def test_running_disk_guard_terminates_only_its_child(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(scale.shutil, "disk_usage") as usage:
            from types import SimpleNamespace
            usage.side_effect = [SimpleNamespace(free=100), SimpleNamespace(free=1)]
            result = scale.guarded([sys.executable, "-c", "import time; time.sleep(30)"], cwd=Path(temporary), env=dict(os.environ), stdout=Path(temporary) / "out", stderr=None, floor=2, disk=Path(temporary))
            self.assertTrue(result["disk_guard_triggered"])
            self.assertNotEqual(result["exit_code"], 0)

    @unittest.skipUnless(os.name == "posix", "owning process-group cleanup is Unix-only")
    def test_runtime_limit_stops_only_the_measured_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = scale.guarded([sys.executable, "-c", "import time; time.sleep(30)"],
                                   cwd=Path(temporary), env=dict(os.environ),
                                   stdout=Path(temporary) / "out", stderr=None,
                                   floor=0, disk=Path(temporary), timeout_seconds=0.01)
            self.assertTrue(result["runtime_limit_triggered"])
            self.assertFalse(result["disk_guard_triggered"])
            self.assertNotEqual(result["exit_code"], 0)

    def test_registry_is_explicit_developer_opt_in(self):
        import tomllib
        tasks = tomllib.loads((ROOT / "tools/tasks.toml").read_text())["task"]
        selected = [task for task in tasks if task["id"].startswith("build.groups.scale.")]
        self.assertEqual(len(selected), len(scale.CASES))
        self.assertEqual({task["entrypoint"][-1] for task in selected}, set(scale.CASES))
        self.assertTrue(all(task["audience"] == ["developer"] for task in selected))
        suites = tomllib.loads((ROOT / "validation/manifest.toml").read_text())["suite"]
        self.assertTrue(all("group-scale.py" not in " ".join(suite["command"]) for suite in suites))
        contracts = next(suite for suite in suites if suite["id"] == "release-contracts")
        self.assertIn("tools/build/group-scale.py", contracts["inputs"])
        self.assertIn("tools/tasks.toml", contracts["inputs"])


if __name__ == "__main__":
    unittest.main()
