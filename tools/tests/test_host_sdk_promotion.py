import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "prepare_host_sdk_promotion",
    ROOT / "tools/release/prepare-host-sdk-promotion.py",
)
PROMOTION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROMOTION)


class HostSdkPromotionTests(unittest.TestCase):
    commit = "1" * 40
    version = (ROOT / "VERSION").read_text().strip()

    def create_stage(self, root):
        stage = root / "stage"
        artifacts = stage / "artifacts" / "fixture"
        artifacts.mkdir(parents=True)
        catalog = json.loads(
            (ROOT / "prns-host/distribution/packages.json").read_text()
        )
        version = self.version
        schema = json.loads(
            (ROOT / catalog["contractSource"]).read_text()
        )
        names = [
            f"personal-rns-{version}-{target['rust']}.{target['archive']}"
            for target in catalog["nativeTargets"]
        ]
        names.extend(
            [
                f"personal-rns-{version}-go.tar.gz",
                f"personal-rns-{version}-swift.tar.gz",
                f"personal-rns-{version}-julia.tar.gz",
                f"personal-rns-{version}-android-jni.zip",
                "source-sdks.json",
            ]
        )
        entries = []
        for index, name in enumerate(names):
            path = artifacts / name
            if name == "source-sdks.json":
                contents = (
                    json.dumps(
                        {
                            "format": 1,
                            "version": version,
                            "commit": self.commit,
                            "tags": {
                                "go": f"prns-host/bindings/go/v{version}",
                                "julia": f"PersonalRns-v{version}",
                                "swift": f"v{version}",
                            },
                        },
                        indent=2,
                        sort_keys=True,
                    )
                    + "\n"
                ).encode()
            else:
                contents = f"fixture-{index}".encode()
            path.write_bytes(contents)
            entries.append(
                {
                    "path": path.relative_to(stage).as_posix(),
                    "bytes": len(contents),
                    "sha256": hashlib.sha256(contents).hexdigest(),
                }
            )
        packages = []
        for package in catalog["packages"]:
            resolved = dict(package)
            if "tag" in resolved:
                resolved["tag"] = resolved["tag"].format(version=version)
            packages.append(resolved)
        release_index = {
            "format": 1,
            "product": catalog["product"],
            "version": version,
            "commit": self.commit,
            "contractAbi": schema["abi"],
            "schemaVersion": schema["schemaVersion"],
            "packages": packages,
            "files": entries,
        }
        (stage / "release-index.json").write_text(
            json.dumps(release_index, indent=2) + "\n"
        )
        for name in ("ADMIN.md", "LICENSE-APACHE", "LICENSE-MIT", "PACKAGE.md"):
            (stage / name).write_text(f"{name}\n")
        return stage

    def test_prepare_selects_every_signed_release_surface(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = self.create_stage(root)
            output = root / "promotion"
            promotion = PROMOTION.prepare(stage, output, self.commit)
            self.assertEqual(
                promotion["releaseTag"], f"host-sdk-v{self.version}"
            )
            self.assertEqual(
                set(promotion["tags"]),
                {
                    f"PersonalRns-v{self.version}",
                    f"host-sdk-v{self.version}",
                    f"prns-host/bindings/go/v{self.version}",
                    f"v{self.version}",
                },
            )
            self.assertEqual(promotion["contractAbi"], 1)
            self.assertEqual(promotion["schemaVersion"], 1)
            self.assertEqual(len(promotion["assets"]), 20)
            self.assertTrue((output / "SHA256SUMS").is_file())
            self.assertTrue((output / "promotion.json").is_file())

    def test_prepare_rejects_an_indexed_asset_that_changed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = self.create_stage(root)
            target = next((stage / "artifacts").rglob("*-go.tar.gz"))
            target.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "changed"):
                PROMOTION.prepare(stage, root / "promotion", self.commit)

    def test_prepare_rejects_a_different_commit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = self.create_stage(root)
            with self.assertRaisesRegex(ValueError, "commit"):
                PROMOTION.prepare(stage, root / "promotion", "2" * 40)

    def test_prepare_rejects_a_self_consistent_wrong_source_tag(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = self.create_stage(root)
            source_manifest = next((stage / "artifacts").rglob("source-sdks.json"))
            document = json.loads(source_manifest.read_text())
            document["tags"]["go"] = "wrong"
            contents = (json.dumps(document, indent=2, sort_keys=True) + "\n").encode()
            source_manifest.write_bytes(contents)
            release_index_path = stage / "release-index.json"
            release_index = json.loads(release_index_path.read_text())
            relative = source_manifest.relative_to(stage).as_posix()
            entry = next(
                entry for entry in release_index["files"] if entry["path"] == relative
            )
            entry["bytes"] = len(contents)
            entry["sha256"] = hashlib.sha256(contents).hexdigest()
            release_index_path.write_text(json.dumps(release_index, indent=2) + "\n")
            with self.assertRaisesRegex(ValueError, "source SDK manifest"):
                PROMOTION.prepare(stage, root / "promotion", self.commit)


if __name__ == "__main__":
    unittest.main()
