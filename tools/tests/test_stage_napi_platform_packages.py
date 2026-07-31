import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "release" / "stage-napi-platform-packages.py"
SPEC = importlib.util.spec_from_file_location("stage_napi_platform_packages", SCRIPT)
STAGER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STAGER)


class StageNapiPlatformPackagesTests(unittest.TestCase):
    def create_packages(self, root):
        version = (ROOT / "VERSION").read_text().strip()
        for platform in STAGER.PLATFORMS:
            directory = root / platform
            directory.mkdir()
            binding = f"personal-rns.{platform}.node"
            (directory / binding).write_bytes(b"binding")
            (directory / "package.json").write_text(
                json.dumps(
                    {
                        "name": f"personal-rns-{platform}",
                        "version": version,
                        "main": binding,
                        "files": [binding],
                    }
                )
            )

    def test_stage_includes_canonical_documents_in_every_allowlist(self):
        with tempfile.TemporaryDirectory() as temporary:
            packages = Path(temporary)
            self.create_packages(packages)
            STAGER.stage(packages)
            STAGER.stage(packages)
            for platform in STAGER.PLATFORMS:
                directory = packages / platform
                manifest = json.loads((directory / "package.json").read_text())
                self.assertEqual(
                    manifest["files"],
                    [manifest["main"], "README.md", "LICENSE-APACHE", "LICENSE-MIT"],
                )
                for name, source in STAGER.DOCUMENTS.items():
                    self.assertEqual((directory / name).read_bytes(), source.read_bytes())

    def test_stage_rejects_incomplete_platform_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            packages = Path(temporary)
            self.create_packages(packages)
            missing = packages / "darwin-arm64"
            for path in missing.iterdir():
                path.unlink()
            missing.rmdir()
            with self.assertRaisesRegex(ValueError, "expected platform packages"):
                STAGER.stage(packages)

    def test_stage_rejects_an_expanded_generated_allowlist(self):
        with tempfile.TemporaryDirectory() as temporary:
            packages = Path(temporary)
            self.create_packages(packages)
            manifest_path = packages / "linux-x64-gnu" / "package.json"
            manifest = json.loads(manifest_path.read_text())
            manifest["files"].append("unexpected.bin")
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "unexpected generated package files"):
                STAGER.stage(packages)


if __name__ == "__main__":
    unittest.main()
