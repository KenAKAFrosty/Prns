from __future__ import annotations

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import types
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "release" / "prnsd-distribution.py"


def load_module() -> types.ModuleType:
    spec = importlib.util.spec_from_file_location("prnsd_distribution", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load prnsd distribution module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


distribution = load_module()


def write_oci_layout(path: Path, platform: str, payload: bytes = b"manifest") -> str:
    digest = f"sha256:{hashlib.sha256(payload).hexdigest()}"
    index = distribution.canonical_json(
        {
            "manifests": [
                {
                    "digest": digest,
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "platform": {
                        "architecture": platform.removeprefix("linux/"),
                        "os": "linux",
                    },
                    "size": len(payload),
                }
            ],
            "schemaVersion": 2,
        }
    )
    with tarfile.open(path, "w") as archive:
        for name, content in (
            ("index.json", index),
            (f"blobs/sha256/{digest.removeprefix('sha256:')}", payload),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return digest


class PrnsdDistributionTests(unittest.TestCase):
    def test_native_archives_are_byte_reproducible_and_self_describing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "prnsd"
            binary.write_bytes(b"test daemon")
            first = root / "first" / "prnsd-0.3.1-x86_64-unknown-linux-gnu.tar.gz"
            second = root / "second" / first.name
            common = {
                "binary": binary,
                "target": "x86_64-unknown-linux-gnu",
                "source_commit": "a" * 40,
                "source_date_epoch": 1_785_330_739,
                "rustc": "rustc 1.96.0 (deadbeef 2026-01-01)",
            }
            for output in (first, second):
                distribution.build_archive(
                    types.SimpleNamespace(output=output, **common)
                )

            self.assertEqual(first.read_bytes(), second.read_bytes())
            with tarfile.open(first, "r:gz") as archive:
                names = archive.getnames()
                identity = json.load(
                    archive.extractfile(
                        "prnsd-0.3.1-x86_64-unknown-linux-gnu/build-identity.json"
                    )
                )
            self.assertIn(
                "prnsd-0.3.1-x86_64-unknown-linux-gnu/THIRD_PARTY_NOTICES.md",
                names,
            )
            self.assertEqual(identity["source_commit"], "a" * 40)
            self.assertEqual(
                identity["features"], ["tokio-host", "observability", "tray"]
            )

    def test_inventory_rejects_any_unrecorded_release_asset(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "one").write_text("one", encoding="utf-8")
            inventory = root / "SHA256SUMS.txt"
            distribution.create_inventory(
                types.SimpleNamespace(assets=root, output=inventory)
            )
            distribution.verify_inventory(
                types.SimpleNamespace(assets=root, inventory=inventory)
            )
            (root / "two").write_text("two", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "differs from the directory"):
                distribution.verify_inventory(
                    types.SimpleNamespace(assets=root, inventory=inventory)
                )

    def test_image_metadata_requires_both_shipping_architectures(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "image.json"
            arguments = types.SimpleNamespace(
                source_commit="b" * 40,
                manifest_digest=f"sha256:{'c' * 64}",
                platform_digest=[f"linux/amd64=sha256:{'d' * 64}"],
                output=output,
            )
            with self.assertRaisesRegex(ValueError, "exactly linux/amd64 and linux/arm64"):
                distribution.write_image_metadata(arguments)

    def test_native_candidate_verification_rejects_post_index_changes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for target in distribution.TARGETS:
                (root / distribution.archive_name("0.3.1", target)).write_bytes(target.encode())
                (root / f"{target}-linkage.txt").write_text("linkage\n", encoding="utf-8")
            (root / "prnsd-0.3.1-source.spdx.json").write_text("{}\n", encoding="utf-8")
            index = root / f"prnsd-candidate-{'a' * 40}.json"
            arguments = types.SimpleNamespace(
                assets=root,
                source_commit="a" * 40,
                repository="KenAKAFrosty/Prns",
                workflow_run_id=41,
                workflow_run_attempt=2,
                output=index,
            )
            distribution.write_candidate_index(arguments)
            verify = types.SimpleNamespace(
                assets=root,
                index=index,
                source_commit="a" * 40,
                repository="KenAKAFrosty/Prns",
                workflow_run_id=41,
            )
            distribution.verify_candidate_index(verify)
            (root / "aarch64-apple-darwin-linkage.txt").write_text(
                "changed\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "producer index"):
                distribution.verify_candidate_index(verify)

    def test_image_candidate_recomputes_platform_digests(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for architecture in ("amd64", "arm64"):
                write_oci_layout(
                    root / f"prnsd-linux-{architecture}.oci.tar",
                    f"linux/{architecture}",
                    architecture.encode(),
                )
                (root / f"prnsd-linux-{architecture}.spdx.json").write_text(
                    "{}\n", encoding="utf-8"
                )
            index = root / f"prnsd-image-candidate-{'b' * 40}.json"
            arguments = types.SimpleNamespace(
                assets=root,
                source_commit="b" * 40,
                repository="KenAKAFrosty/Prns",
                workflow_run_id=52,
                workflow_run_attempt=1,
                output=index,
            )
            distribution.write_image_candidate_index(arguments)
            distribution.verify_image_candidate_index(
                types.SimpleNamespace(
                    assets=root,
                    index=index,
                    source_commit="b" * 40,
                    repository="KenAKAFrosty/Prns",
                    workflow_run_id=52,
                )
            )
            value = json.loads(index.read_text(encoding="utf-8"))
            value["platform_digests"]["linux/amd64"] = f"sha256:{'0' * 64}"
            index.write_bytes(distribution.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "platform digests"):
                distribution.verify_image_candidate_index(
                    types.SimpleNamespace(
                        assets=root,
                        index=index,
                        source_commit="b" * 40,
                        repository="KenAKAFrosty/Prns",
                        workflow_run_id=52,
                    )
                )

    def test_railway_contract_exposes_write_once_announcement_controls(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "railway-template-contract-v0.3.1.json"
            distribution.write_railway_contract(
                types.SimpleNamespace(
                    source_commit="c" * 40,
                    image_digest=f"sha256:{'d' * 64}",
                    output=output,
                )
            )
            contract = json.loads(output.read_text(encoding="utf-8"))
            self.assertTrue(contract["bootstrap"]["write_once"])
            controls = contract["bootstrap"]["operator_environment"]
            self.assertEqual(
                controls["PRNSD_BACKBONE_DISCOVERABLE"],
                {"allowed": ["Yes", "No"], "default": "Yes"},
            )
            self.assertEqual(
                controls["PRNSD_NODE_PAGE_ANNOUNCE"],
                {"allowed": ["Yes", "No"], "default": "Yes"},
            )
            self.assertEqual(
                controls["PRNSD_NODE_PAGE_ANNOUNCE_INTERVAL"],
                {"default": "360", "unit": "minutes"},
            )

    def test_suite_record_binds_every_inventoried_asset(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets = root / "assets"
            custody = root / "custody"
            release = root / "release"
            assets.mkdir()
            custody.mkdir()
            release.mkdir()
            commit = "c" * 40
            manifest_digest = f"sha256:{'d' * 64}"
            platform_digests = {
                "linux/amd64": f"sha256:{'e' * 64}",
                "linux/arm64": f"sha256:{'f' * 64}",
            }
            for target in distribution.TARGETS:
                (assets / distribution.archive_name("0.3.1", target)).write_bytes(
                    target.encode()
                )
                (assets / f"{target}-linkage.txt").write_text(
                    "linkage\n", encoding="utf-8"
                )
            for name in (
                "prnsd-0.3.1-source.spdx.json",
                "prnsd-linux-amd64.spdx.json",
                "prnsd-linux-arm64.spdx.json",
            ):
                (assets / name).write_text("{}\n", encoding="utf-8")
            for name in (
                "prnsd-native-attestation-v0.3.1.json",
                "prnsd-image-attestation-v0.3.1.json",
                "prns-flasher-candidate-v0.3.1-signed.tar.gz",
            ):
                (assets / name).write_text("evidence\n", encoding="utf-8")
            distribution.write_image_metadata(
                types.SimpleNamespace(
                    source_commit=commit,
                    manifest_digest=manifest_digest,
                    platform_digest=[
                        f"{platform}={digest}"
                        for platform, digest in platform_digests.items()
                    ],
                    output=assets / "prnsd-image-v0.3.1.json",
                )
            )
            distribution.write_railway_contract(
                types.SimpleNamespace(
                    source_commit=commit,
                    image_digest=manifest_digest,
                    output=assets / "railway-template-contract-v0.3.1.json",
                )
            )
            (assets / f"prnsd-candidate-{commit}.json").write_bytes(
                distribution.canonical_json(
                    {
                        "source_commit": commit,
                        "version": "0.3.1",
                        "workflow": {
                            "path": ".github/workflows/prnsd-candidate.yml"
                        },
                    }
                )
            )
            (assets / f"prnsd-image-candidate-{commit}.json").write_bytes(
                distribution.canonical_json(
                    {
                        "platform_digests": platform_digests,
                        "source_commit": commit,
                        "version": "0.3.1",
                        "workflow": {
                            "path": ".github/workflows/prnsd-image-candidate.yml"
                        },
                    }
                )
            )
            inventory = custody / "SHA256SUMS.txt"
            distribution.create_inventory(
                types.SimpleNamespace(assets=assets, output=inventory)
            )
            record = custody / "release-record-v0.3.1.json"
            distribution.write_suite_record(
                types.SimpleNamespace(
                    assets=assets,
                    inventory=inventory,
                    source_commit=commit,
                    output=record,
                )
            )
            for path in assets.iterdir():
                shutil.copy2(path, release / path.name)
            for path in (inventory, record):
                shutil.copy2(path, release / path.name)
                (release / f"{path.name}.minisig").write_text(
                    "signature\n", encoding="utf-8"
                )
            shutil.copy2(
                ROOT / "release/keys/minisign.pub", release / "minisign.pub"
            )
            verify = types.SimpleNamespace(
                assets=release,
                source_commit=commit,
                image_digest=manifest_digest,
            )
            distribution.verify_suite_release(verify)
            (
                release
                / "public-review-v0.3.1-run-71-attempt-2.json"
            ).write_text("{}\n", encoding="utf-8")
            (
                release / "qualification-evidence-v0.3.1.tar.gz"
            ).write_bytes(b"separately signed flasher evidence")
            (
                release / "deployment-qualification-v0.3.1.json"
            ).write_text("{}\n", encoding="utf-8")
            distribution.verify_suite_release(verify)
            (release / "unexpected").write_text("not inventoried\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not exact"):
                distribution.verify_suite_release(verify)
            (release / "unexpected").unlink()
            (release / "prnsd-linux-arm64.spdx.json").write_text(
                '{"changed": true}\n', encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "checksum differs"):
                distribution.verify_suite_release(verify)


if __name__ == "__main__":
    unittest.main()
