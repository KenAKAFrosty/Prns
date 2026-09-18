from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CHECK = ROOT / "validation" / "hygiene" / "application-boundary.py"


class ApplicationBoundaryTests(unittest.TestCase):
    def run_check(self, files: dict[str, str]) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative, content in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(CHECK), "--root", str(root)],
                check=False,
                capture_output=True,
                text=True,
            )

    @staticmethod
    def base_files() -> dict[str, str]:
        return {
            "Cargo.toml": '[workspace]\nmembers = ["base"]\nexclude = ["applications"]\n',
            "base/Cargo.toml": '[package]\nname = "base"\nversion = "0.1.0"\n',
            "applications/app/Cargo.toml": (
                '[package]\nname = "app"\nversion = "0.1.0"\n'
                '[dependencies]\nbase = { path = "../../base" }\n'
            ),
        }

    def test_allows_the_application_to_depend_on_base_packages(self) -> None:
        result = self.run_check(self.base_files())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("APPLICATION_DEPENDENCY_BOUNDARY_OK", result.stdout)

    def test_rejects_a_base_cargo_path_into_applications(self) -> None:
        files = self.base_files()
        files["base/Cargo.toml"] += (
            '[dependencies]\napp = { path = "../applications/app" }\n'
        )
        result = self.run_check(files)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("resolves into applications", result.stderr)

    def test_rejects_a_base_npm_path_into_applications(self) -> None:
        files = self.base_files()
        files["base-js/package.json"] = (
            '{"name":"base-js","dependencies":{"app":"file:../applications/app-js"}}'
        )
        files["applications/app-js/package.json"] = '{"name":"app-js"}'
        result = self.run_check(files)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("dependencies.app", result.stderr)

    def test_rejects_a_direct_base_source_import(self) -> None:
        files = self.base_files()
        files["base-js/package.json"] = '{"name":"base-js"}'
        files["base-js/index.ts"] = 'export { value } from "../applications/app-js/value";\n'
        files["applications/app-js/value.ts"] = "export const value = 1;\n"
        result = self.run_check(files)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("imports application source", result.stderr)

    def test_requires_the_root_workspace_exclusion(self) -> None:
        files = self.base_files()
        files["Cargo.toml"] = '[workspace]\nmembers = ["base"]\n'
        result = self.run_check(files)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("workspace.exclude", result.stderr)


if __name__ == "__main__":
    unittest.main()
