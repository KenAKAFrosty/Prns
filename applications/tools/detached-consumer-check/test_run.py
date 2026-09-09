from __future__ import annotations

import importlib.util
import hashlib
import io
import json
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock


MODULE_PATH = pathlib.Path(__file__).with_name("run.py")
SPEC = importlib.util.spec_from_file_location("detached_consumer_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not load {MODULE_PATH}")
mobility = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = mobility
SPEC.loader.exec_module(mobility)


class DetachedConsumerCheckTests(unittest.TestCase):
    @staticmethod
    def cargo_compatibility(paths: dict[str, str]) -> dict[str, object]:
        return {
            "prns": {
                "rustPackages": {
                    "direct": list(paths),
                    "source": [
                        {"name": name, "path": path, "version": "0.0.0"}
                        for name, path in paths.items()
                    ],
                }
            }
        }

    @staticmethod
    def npm_compatibility() -> dict[str, object]:
        return {
            "prns": {
                "javascriptContract": {
                    "package": "personal-rns",
                    "sourcePath": "prns-js",
                    "consumers": ["prns/app/package.json", "sdk/expo/package.json"],
                }
            }
        }

    def test_compatibility_manifest_requires_canonical_bluetooth_uuid(self) -> None:
        document = json.loads(mobility.COMPATIBILITY_PATH.read_text(encoding="utf-8"))
        for bluetooth in (
            None,
            {"serviceUuid": "37145b00-442d-4a94-917f-8f42c5da28e3"},
        ):
            mutated = json.loads(json.dumps(document))
            if bluetooth is None:
                mutated["prns"].pop("bluetoothAuto")
            else:
                mutated["prns"]["bluetoothAuto"] = bluetooth
            with self.subTest(bluetooth=bluetooth), tempfile.TemporaryDirectory() as temporary:
                path = pathlib.Path(temporary) / "compatibility.json"
                path.write_text(json.dumps(mutated), encoding="utf-8")
                with mock.patch.object(mobility, "COMPATIBILITY_PATH", path):
                    with self.assertRaisesRegex(
                        mobility.QualificationFailure, "bluetoothAuto"
                    ):
                        mobility.load_compatibility()

    def test_cargo_rewrite_replaces_only_reviewed_base_packages(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            crate = applications / "service"
            crate.mkdir(parents=True)
            dependencies = {
                "personal-rns": "personal-rns",
                "prns-core": "prns-core",
                "prns-ffi": "prns-ffi",
                "prns-host": "prns-host/core",
                "prns-host-snapshot": "prns-host/impls/snapshot",
            }
            reviewed, _ = mobility.rust_packages(mobility.load_compatibility())
            self.assertTrue(set(dependencies).issubset(reviewed))
            declarations = []
            for package, relative in dependencies.items():
                source = repository / relative
                source.mkdir(parents=True)
                (source / "Cargo.toml").write_text(
                    f'[package]\nname = "{package}"\nversion = "0.0.0"\n',
                    encoding="utf-8",
                )
                path = os.path.relpath(source, crate)
                options = (
                    ", default-features = false, optional = true"
                    if package == "prns-ffi"
                    else ""
                )
                declarations.append(f'{package} = {{ path = "{path}"{options} }}')
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "fixture"\nversion = "0.0.0"\n\n[dependencies]\n'
                + "\n".join(declarations)
                + "\n",
                encoding="utf-8",
            )

            compatibility = self.cargo_compatibility(dependencies)
            source_only = json.loads(json.dumps(compatibility))
            source_only["prns"]["rustPackages"]["direct"].remove("prns-ffi")
            with self.assertRaisesRegex(
                mobility.QualificationFailure, "unreviewed external Cargo dependency prns-ffi"
            ):
                mobility.cargo_rewrite_plan(applications, repository, source_only)

            rewrites = mobility.cargo_rewrite_plan(
                applications,
                repository,
                compatibility,
            )
            revision = "1" * 40
            mobility.rewrite_cargo_dependencies(
                applications,
                rewrites,
                "file:///exact/prns",
                revision,
            )

            rendered = (crate / "Cargo.toml").read_text(encoding="utf-8")
            self.assertNotIn("path =", rendered)
            self.assertEqual(rendered.count('git = "file:///exact/prns"'), 5)
            self.assertEqual(rendered.count(f'rev = "{revision}"'), 5)
            self.assertIn("default-features = false, optional = true", rendered)
            mobility.reject_external_cargo_paths(applications)

    def test_cargo_scanner_rejects_unhandled_dependency_table(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            crate = applications / "service"
            dependency = repository / "personal-rns"
            crate.mkdir(parents=True)
            dependency.mkdir()
            (dependency / "Cargo.toml").write_text(
                '[package]\nname = "personal-rns"\nversion = "0.0.0"\n',
                encoding="utf-8",
            )
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "fixture"\nversion = "0.0.0"\n'
                '[dependencies.personal-rns]\npath = "../../personal-rns"\n',
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                mobility.QualificationFailure, "unsupported or unreviewed declaration"
            ):
                mobility.cargo_rewrite_plan(
                    applications,
                    repository,
                    self.cargo_compatibility({"personal-rns": "personal-rns"}),
                )

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
                mobility.QualificationFailure,
                "Cargo path escapes detached applications",
            ):
                mobility.reject_external_cargo_paths(applications)

    def test_cargo_lock_refresh_only_allows_exact_git_sources(self) -> None:
        revision = "1" * 40
        git_url = "file:///exact/prns.git"
        self.assertTrue(
            mobility.exact_git_source(
                f"git+{git_url}?rev={revision}", git_url, revision, resolved=False
            )
        )
        before = b"""version = 4

[[package]]
name = "personal-rns"
version = "0.0.0"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"
"""
        after = f"""version = 4

[[package]]
name = "personal-rns"
version = "0.0.0"
source = "git+file:///exact/prns.git?rev={revision}#{revision}"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"
"""
        with tempfile.TemporaryDirectory() as temporary:
            lock = pathlib.Path(temporary) / "Cargo.lock"
            lock.write_text(after, encoding="utf-8")
            packages = {
                "personal-rns": mobility.RustPackage(
                    name="personal-rns",
                    path=pathlib.PurePosixPath("personal-rns"),
                    version="0.0.0",
                )
            }
            mobility.validate_cargo_lock_refresh(
                before, lock, packages, git_url, revision
            )
            lock.write_text(
                after.replace('checksum = "abc"', 'checksum = "def"'), encoding="utf-8"
            )
            with self.assertRaisesRegex(
                mobility.QualificationFailure, "changed serde@1.0.0"
            ):
                mobility.validate_cargo_lock_refresh(
                    before, lock, packages, git_url, revision
                )

    def test_cargo_metadata_resolves_optional_dependency_sources(self) -> None:
        completed = subprocess.CompletedProcess(
            args=["cargo", "metadata"], returncode=0, stdout="{}"
        )
        with mock.patch.object(mobility, "run", return_value=completed) as invoked:
            self.assertEqual(
                mobility.cargo_metadata(pathlib.Path("/applications"), {}, locked=True),
                {},
            )

        arguments = tuple(invoked.call_args.args[0])
        self.assertEqual(arguments[:3], ("cargo", "metadata", "--all-features"))
        self.assertIn("--locked", arguments)

    def test_npm_workspaces_share_one_export_local_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            personal_rns = repository / "prns-js"
            personal_rns.mkdir()
            (personal_rns / "package.json").write_text(
                '{"name":"personal-rns","version":"0.0.0"}\n', encoding="utf-8"
            )
            runtime_selection = "file:../../vendor/ubrn/packages/ubjs-core.tgz"
            for relative in (pathlib.Path("prns/app"), pathlib.Path("sdk/expo")):
                package = applications / relative
                package.mkdir(parents=True)
                selection = f"file:{os.path.relpath(personal_rns, package)}"
                (package / "package.json").write_text(
                    json.dumps(
                        {
                            "name": str(relative),
                            "dependencies": {
                                "personal-rns": selection,
                                "@ubjs/core": runtime_selection,
                            },
                        }
                    ),
                    encoding="utf-8",
                )
            bindings = applications / "prns/native-composition/bindings/package.json"
            bindings.parent.mkdir(parents=True)
            binding_manifest = json.dumps({
                "name": "@prns-internal/native-bindings",
                "peerDependencies": {"personal-rns": "*"},
                "dependencies": {"@ubjs/core": "0.31.0-5"},
            })
            bindings.write_text(binding_manifest, encoding="utf-8")
            artifact = applications / "vendor" / "personal-rns.tgz"
            artifact.parent.mkdir()
            artifact.write_bytes(b"fixture")

            rewrites = mobility.npm_rewrite_plan(
                applications, repository, self.npm_compatibility()
            )
            selection = mobility.rewrite_npm_dependencies(
                applications, rewrites, artifact
            )

            self.assertEqual(selection, "file:../../vendor/personal-rns.tgz")
            self.assertEqual(bindings.read_text(encoding="utf-8"), binding_manifest)
            for relative in ("prns/app", "sdk/expo"):
                package = json.loads((applications / relative / "package.json").read_text())
                self.assertEqual(package["dependencies"]["@ubjs/core"], runtime_selection)
            mobility.reject_external_npm_paths(applications)

    def test_personal_rns_pack_preserves_committed_runtime_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = pathlib.Path(temporary)
            applications = repository / "applications"
            runtime = applications / "vendor/ubrn/packages/ubjs-core.tgz"
            runtime.parent.mkdir(parents=True)
            runtime.write_bytes(b"pinned runtime archive")
            artifact = applications / "vendor/personal-rns.tgz"
            metadata = {
                "name": "personal-rns", "version": "0.0.0",
                "exports": {"./contract": "./contract.js"},
            }
            with tarfile.open(artifact, "w:gz") as archive:
                for name, data in {
                    "package/package.json": json.dumps(metadata).encode(),
                    "package/contract.js": b"export {};",
                }.items():
                    member = tarfile.TarInfo(name)
                    member.size = len(data)
                    archive.addfile(member, io.BytesIO(data))
            compatibility = self.npm_compatibility()
            javascript = compatibility["prns"]["javascriptContract"]
            javascript.update({
                "version": "0.0.0", "subpath": "./contract",
                "requiredFiles": ["package/package.json", "package/contract.js"],
                "artifactSha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
            })
            packed = subprocess.CompletedProcess(
                args=["npm", "pack"], returncode=0,
                stdout=json.dumps([{"filename": artifact.name}]),
            )
            with mock.patch.object(mobility, "run", return_value=packed):
                result, digest, _ = mobility.build_javascript_artifact(
                    repository, applications, compatibility, {},
                )
            self.assertEqual(result, artifact)
            self.assertEqual(digest, javascript["artifactSha256"])
            self.assertEqual(runtime.read_bytes(), b"pinned runtime archive")

    def test_environment_retains_only_explicit_generator_cache_override(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            work = pathlib.Path(temporary)
            cache = work / "shared generator cache"
            inherited = {
                "PATH": os.environ.get("PATH", ""),
                "PRNS_UBRN_CACHE": os.fspath(cache),
                "PRNS_COMPATIBILITY_PRNS_ROOT": "/unreviewed/source",
                "CARGO_TARGET_DIR": "/unreviewed/target",
                "NPM_CONFIG_USERCONFIG": "/unreviewed/.npmrc",
                "NODE_OPTIONS": "--require=/unreviewed/bootstrap.js",
            }
            with mock.patch.dict(os.environ, inherited, clear=True):
                environment = mobility.controlled_environment(
                    work, work / "applications", mobility.load_compatibility(),
                )
            self.assertEqual(environment["PRNS_UBRN_CACHE"], os.fspath(cache.resolve()))
            self.assertNotIn("PRNS_COMPATIBILITY_PRNS_ROOT", environment)
            self.assertNotIn("NODE_OPTIONS", environment)
            self.assertEqual(environment["CARGO_TARGET_DIR"], os.fspath(work / "applications/target"))
            self.assertEqual(environment["NPM_CONFIG_USERCONFIG"], os.fspath(work / "npm-user.npmrc"))

    def test_npm_scanner_rejects_local_override_outside_export(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            applications = pathlib.Path(temporary) / "applications"
            package = applications / "app"
            package.mkdir(parents=True)
            (package / "package.json").write_text(
                json.dumps({"overrides": {"example": "link:../../../outside"}}),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                mobility.QualificationFailure, "npm local path escapes"
            ):
                mobility.reject_external_npm_paths(applications)

    def test_npm_lock_content_ignores_only_derived_classification(self) -> None:
        original = {
            "version": "1.2.3",
            "resolved": "https://registry.example/package.tgz",
            "integrity": "sha512-example",
            "dev": True,
        }
        reclassified = {
            "version": "1.2.3",
            "resolved": "https://registry.example/package.tgz",
            "integrity": "sha512-example",
            "devOptional": True,
        }
        changed = {**reclassified, "version": "1.2.4"}

        self.assertTrue(mobility.same_npm_package_content(original, reclassified))
        self.assertFalse(mobility.same_npm_package_content(original, changed))

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
                mobility.QualificationFailure,
                "tracked application symlinks are forbidden",
            ):
                mobility.reject_tracked_symlinks(repository)


if __name__ == "__main__":
    unittest.main()
