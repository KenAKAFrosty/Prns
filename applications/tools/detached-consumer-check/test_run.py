from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("run.py")
SPEC = importlib.util.spec_from_file_location("detached_consumer_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not load {MODULE_PATH}")
mobility = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = mobility
SPEC.loader.exec_module(mobility)


class DetachedConsumerCheckTests(unittest.TestCase):
    def test_cargo_rewrite_replaces_only_reviewed_base_packages(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            crate = applications / "service"
            crate.mkdir(parents=True)
            dependencies = {
                "personal-rns": "personal-rns",
                "prns-core": "prns-core",
                "prns-host": "prns-host/core",
                "prns-host-snapshot": "prns-host/impls/snapshot",
            }
            declarations = []
            for package, relative in dependencies.items():
                source = repository / relative
                source.mkdir(parents=True)
                (source / "Cargo.toml").write_text(
                    f'[package]\nname = "{package}"\nversion = "0.0.0"\n',
                    encoding="utf-8",
                )
                path = os.path.relpath(source, crate)
                declarations.append(f'{package} = {{ path = "{path}" }}')
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "fixture"\nversion = "0.0.0"\n\n[dependencies]\n'
                + "\n".join(declarations)
                + "\n",
                encoding="utf-8",
            )

            rewrites = mobility.cargo_rewrite_plan(applications, repository)
            revision = "1" * 40
            mobility.rewrite_cargo_dependencies(
                applications,
                rewrites,
                "file:///exact/prns",
                revision,
            )

            rendered = (crate / "Cargo.toml").read_text(encoding="utf-8")
            self.assertNotIn("path =", rendered)
            self.assertEqual(rendered.count('git = "file:///exact/prns"'), 4)
            self.assertEqual(rendered.count(f'rev = "{revision}"'), 4)
            mobility.reject_external_cargo_paths(applications)

    def test_external_cargo_path_is_rejected_after_staging(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            applications = pathlib.Path(temporary) / "applications"
            crate = applications / "service"
            crate.mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "fixture"\nversion = "0.0.0"\n'
                '[dependencies]\npersonal-rns = { path = "../../../personal-rns" }\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                mobility.QualificationFailure, "Cargo path escapes detached applications"
            ):
                mobility.reject_external_cargo_paths(applications)

    def test_npm_workspaces_share_one_export_local_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            personal_rns = repository / "prns-js"
            personal_rns.mkdir()
            (personal_rns / "package.json").write_text(
                '{"name":"personal-rns","version":"0.0.0"}\n', encoding="utf-8"
            )
            for relative in (pathlib.Path("prns/app"), pathlib.Path("sdk/expo")):
                package = applications / relative
                package.mkdir(parents=True)
                selection = f"file:{os.path.relpath(personal_rns, package)}"
                (package / "package.json").write_text(
                    json.dumps(
                        {
                            "name": str(relative),
                            "dependencies": {"personal-rns": selection},
                        }
                    ),
                    encoding="utf-8",
                )
            artifact = applications / "vendor" / "personal-rns.tgz"
            artifact.parent.mkdir()
            artifact.write_bytes(b"fixture")

            rewrites = mobility.npm_rewrite_plan(applications, repository)
            selection = mobility.rewrite_npm_dependencies(applications, rewrites, artifact)

            self.assertEqual(selection, "file:../../vendor/personal-rns.tgz")
            mobility.reject_external_npm_paths(applications)

    def test_tracked_symlink_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            applications.mkdir()
            target = applications / "target.txt"
            target.write_text("target", encoding="utf-8")
            try:
                (applications / "link.txt").symlink_to(target.name)
            except OSError as error:
                self.skipTest(f"symlinks are unavailable: {error}")
            subprocess.run(("git", "init", "--quiet"), cwd=repository, check=True)
            subprocess.run(("git", "add", "applications"), cwd=repository, check=True)

            with self.assertRaisesRegex(
                mobility.QualificationFailure, "tracked application symlinks are forbidden"
            ):
                mobility.reject_tracked_symlinks(repository)


if __name__ == "__main__":
    unittest.main()
