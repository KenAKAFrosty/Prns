from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest

CHECK = Path(__file__).resolve().parents[1] / "hygiene/application-boundary.py"
spec = importlib.util.spec_from_file_location("application_boundary", CHECK)
assert spec is not None and spec.loader is not None
boundary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boundary)


class ApplicationBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.write("validation/manifest.toml", '[registry]\ncargo_manifests=["Cargo.toml", "base/Cargo.toml", "applications/app/Cargo.toml"]\n')
        self.write("Cargo.toml", '[workspace]\nmembers=["base", "applications/app"]\n')
        self.write("base/Cargo.toml", '[package]\nname="base"\nversion="0.1.0"\n')
        self.write("applications/app/Cargo.toml", '[package]\nname="app"\nversion="0.1.0"\n[dependencies]\nbase={path="../../base"}\n')
        self.write("applications/app-js/package.json", '{"name":"@prns/app"}')

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    def append(self, name, content):
        path = self.root / name
        path.write_text(path.read_text() + content)

    def test_application_consumers_and_test_dependencies_do_not_impose_workspace_layout(self):
        self.append("base/Cargo.toml", '[dev-dependencies]\napp={path="../applications/app"}\n')
        self.write("base/tests/consumer.rs", 'include!("../../applications/app/src/lib.rs");')
        self.write("base-js/package.json", '{"name":"base-js","devDependencies":{"@prns/app":"workspace:*"}}')
        self.write("base-js/src/consumer.test.ts", 'import app from "@prns/app";')
        self.assertEqual(boundary.check(self.root), [])

    def test_target_build_dependency_uses_normalized_path(self):
        self.append("base/Cargo.toml", '[target.\'cfg(unix)\'.build-dependencies]\napp={path="../base/../applications/app"}\n')
        self.assertIn("target.cfg(unix).build-dependencies.app.path", "\n".join(boundary.check(self.root)))

    def test_workspace_dependency_and_patch_are_checked(self):
        self.append("Cargo.toml", '[workspace.dependencies]\napp={path="applications/app"}\n[patch.crates-io]\nbase={path="applications/app"}\n')
        errors = boundary.check(self.root)
        self.assertEqual(len(errors), 2)
        self.assertTrue(any("workspace.dependencies" in error for error in errors))
        self.assertTrue(any("patch.crates-io" in error for error in errors))

    def test_dependency_symlink_cannot_hide_app_target(self):
        (self.root / "base/shared").symlink_to(self.root / "applications/app", target_is_directory=True)
        self.append("base/Cargo.toml", '[dependencies]\napp={path="shared"}\n')
        self.assertEqual(len(boundary.check(self.root)), 1)

    def test_missing_registered_manifest_fails_closed(self):
        (self.root / "base/Cargo.toml").unlink()
        with self.assertRaises(FileNotFoundError):
            boundary.check(self.root)

    def test_unregistered_cargo_file_is_not_an_implicit_production_package(self):
        self.write("scratch/Cargo.toml", '[dependencies]\napp={path="../applications/app"}\n')
        self.assertEqual(boundary.check(self.root), [])

    def test_npm_local_optional_and_named_peer_dependencies(self):
        self.write("sdk/package.json", '{"name":"sdk","optionalDependencies":{"local":"link:../applications/app-js"},"peerDependencies":{"@prns/app":"*"}}')
        self.assertEqual(len(boundary.check(self.root)), 2)

    def test_file_dependency_without_dot_prefix_is_still_local(self):
        self.write("package.json", '{"name":"sdk","dependencies":{"alias":"file:applications/app-js"}}')
        self.assertEqual(len(boundary.check(self.root)), 1)

    def test_rust_include_without_dot_prefix_resolves_from_source(self):
        (self.root / "base/src").mkdir()
        (self.root / "base/src/app").symlink_to(self.root / "applications/app", target_is_directory=True)
        self.write("base/src/lib.rs", 'include!("app/lib.rs");')
        self.assertEqual(len(boundary.check(self.root)), 1)

    def test_npm_alias_to_app_package_is_rejected(self):
        self.write("sdk/package.json", '{"name":"sdk","dependencies":{"alias":"npm:@prns/app@^1"}}')
        self.assertEqual(len(boundary.check(self.root)), 1)

    def test_production_sources_cannot_bypass_manifest_dependency(self):
        self.write("base/src/lib.rs", 'include!("../../applications/app/src/lib.rs");')
        self.write("sdk/package.json", '{"name":"sdk"}')
        self.write("sdk/src/index.ts", 'export { app } from "../../applications/app-js/value";\nconst lazy = import("@prns/app/subpath");')
        errors = boundary.check(self.root)
        self.assertEqual(len(errors), 3)
        self.assertTrue(any("imports application package" in error for error in errors))

    def test_explicit_library_path_into_app_is_rejected(self):
        self.append("base/Cargo.toml", '[lib]\npath="../applications/app/src/lib.rs"\n')
        self.assertEqual(len(boundary.check(self.root)), 1)

    def test_generated_caches_and_other_source_packages_are_allowed(self):
        self.write("sdk/package.json", '{"name":"sdk","dependencies":{"base":"file:../base"}}')
        self.write("sdk/src/index.ts", 'export { value } from "../shared/value";')
        self.write("sdk/node_modules/ignored/package.json", '{"dependencies":{"app":"file:../../../applications/app-js"}}')
        self.write("sdk/target/bad.rs", 'include!("../../applications/app/src/lib.rs");')
        self.assertEqual(boundary.check(self.root), [])


if __name__ == "__main__":
    unittest.main()
