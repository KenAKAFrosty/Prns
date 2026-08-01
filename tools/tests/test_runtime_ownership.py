import json
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def reachable_package_names(manifest):
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(manifest)],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    packages = {package["id"]: package["name"] for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root = next(package for package in metadata["packages"] if Path(package["manifest_path"]) == manifest.resolve())
    pending = [root["id"]]
    reachable = set()
    while pending:
        package_id = pending.pop()
        if package_id in reachable:
            continue
        reachable.add(package_id)
        pending.extend(dependency["pkg"] for dependency in nodes[package_id]["deps"])
    return {packages[package_id] for package_id in reachable}


def binding_sources(binding):
    suffixes = {"dotnet": {".cs"}, "python": {".py"}, "go": {".go"}, "swift": {".swift"}, "jvm": {".kt"}, "julia": {".jl"}}[binding]
    root = ROOT / "prns-host" / "bindings" / binding
    return [path for path in root.rglob("*") if path.is_file() and path.suffix in suffixes and not {"build", "obj", ".build"}.intersection(path.parts)]


class RuntimeOwnershipTests(unittest.TestCase):
    def test_napi_does_not_cross_the_c_capsule(self):
        dependencies = reachable_package_names(ROOT / "prns-napi" / "Cargo.toml")
        self.assertIn("prns-host-native", dependencies)
        self.assertNotIn("prns-host-c", dependencies)

    def test_wasm_does_not_depend_on_the_native_host(self):
        dependencies = reachable_package_names(ROOT / "prns-wasm" / "Cargo.toml")
        self.assertIn("prns-host-cooperative", dependencies)
        self.assertNotIn("prns-host-native", dependencies)
        self.assertNotIn("prns-host-c", dependencies)

    def test_c_capsule_owns_the_native_host(self):
        dependencies = reachable_package_names(ROOT / "prns-host" / "abi" / "c" / "Cargo.toml")
        self.assertIn("prns-host-native", dependencies)

    def test_stable_sdk_bindings_enter_through_c_only(self):
        for binding in ("dotnet", "python", "go", "swift", "jvm", "julia"):
            content = "\n".join(path.read_text(errors="ignore") for path in binding_sources(binding))
            with self.subTest(binding=binding):
                self.assertIn("prns_host_", content)
                self.assertNotIn("prns-host-native", content)
                self.assertNotIn("prns_host_native", content)


if __name__ == "__main__":
    unittest.main()
