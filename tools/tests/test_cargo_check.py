from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "tools" / "repo" / "cargo-check.py"
SPEC = importlib.util.spec_from_file_location("cargo_check", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
cargo_check = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = cargo_check
SPEC.loader.exec_module(cargo_check)


class CargoCheckTests(unittest.TestCase):
    def write_repository(self, root: Path, manifest: str) -> Path:
        for workspace in (root, root / "host", root / "firmware"):
            workspace.mkdir(parents=True, exist_ok=True)
            (workspace / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
            (workspace / "Cargo.lock").write_text("version = 3\n", encoding="utf-8")
        path = root / "validation" / "manifest.toml"
        path.parent.mkdir()
        path.write_text(f"schema = 1\n{manifest}", encoding="utf-8")
        return path

    def test_plan_checks_every_registered_workspace_not_delegated(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.write_repository(
                root,
                """
[registry]
cargo_lock_workspaces = [".", "host", "firmware"]

[[registry.cargo_check_delegations]]
workspace = "firmware"
suite = "firmware-build"
reason = "Uses its real target."

[[suite]]
id = "firmware-build"
tiers = ["pr"]
""",
            )

            plan = cargo_check.load_plan(root, manifest)

            self.assertEqual(plan.workspaces, (".", "host"))
            self.assertEqual(
                plan.delegations,
                (
                    cargo_check.CargoCheckDelegation(
                        "firmware", "firmware-build", "Uses its real target."
                    ),
                ),
            )

    def test_plan_rejects_a_delegation_without_pr_coverage(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.write_repository(
                root,
                """
[registry]
cargo_lock_workspaces = [".", "host", "firmware"]

[[registry.cargo_check_delegations]]
workspace = "firmware"
suite = "firmware-build"
reason = "Uses its real target."

[[suite]]
id = "firmware-build"
tiers = ["release"]
""",
            )

            with self.assertRaisesRegex(
                cargo_check.CargoCheckConfigurationError,
                "must run in the PR tier",
            ):
                cargo_check.load_plan(root, manifest)

    @mock.patch.object(cargo_check.subprocess, "run")
    def test_workspace_check_uses_all_targets_and_a_shared_target(self, run: mock.Mock) -> None:
        run.return_value = mock.Mock(returncode=0, stdout="", stderr="")
        root = Path("/repository")
        target = root / "target" / "repo-cargo-check" / "linux"

        result = cargo_check.run_workspace(root, "host", 2, target)

        self.assertEqual(result.returncode, 0)
        command = run.call_args.args[0]
        self.assertEqual(
            command,
            (
                "cargo",
                "check",
                "--workspace",
                "--all-targets",
                "--locked",
                "--jobs",
                "2",
            ),
        )
        self.assertEqual(run.call_args.kwargs["cwd"], root / "host")
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_TARGET_DIR"], str(target))

    def test_pre_push_runs_the_canonical_repository_check(self) -> None:
        hook = (ROOT / ".githooks" / "pre-push").read_text(encoding="utf-8")

        self.assertIn('"${repo_root}/tools/prns" repo cargo-check', hook)

    def test_ci_checks_all_host_operating_systems_on_trunk_and_main(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("branches: [trunk, main]", workflow)
        self.assertIn("runner: ubuntu-24.04", workflow)
        self.assertIn("runner: macos-latest", workflow)
        self.assertIn("runner: windows-latest", workflow)
        self.assertIn("CARGO_CHECK_RESULT: ${{ needs.cargo-check.result }}", workflow)


if __name__ == "__main__":
    unittest.main()
