from __future__ import annotations

import gzip
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
VERSION = (ROOT / "VERSION").read_text(encoding="utf-8").strip()


def load_module() -> types.ModuleType:
    spec = importlib.util.spec_from_file_location("prnsd_distribution", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load prnsd distribution module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


distribution = load_module()


def gzip_layer(members: list[tuple[str, bytes]]) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as layer:
        for name, content in members:
            info = tarfile.TarInfo(name)
            info.size = len(content)
            layer.addfile(info, io.BytesIO(content))
    return gzip.compress(buffer.getvalue(), mtime=0)


def image_members(architecture: str, source: bytes) -> list[tuple[str, bytes]]:
    checksum = f"{hashlib.sha256(source).hexdigest()}  source.zip\n".encode()
    return [
        ("usr/local/bin/prnsd", architecture.encode()),
        ("usr/share/prnsd/source.zip", source),
        ("usr/share/prnsd/source.zip.sha256", checksum),
    ]


def write_oci_layout_layers(
    path: Path, platform: str, layers_members: list[list[tuple[str, bytes]]]
) -> str:
    layers = [gzip_layer(members) for members in layers_members]
    layer_digests = [f"sha256:{hashlib.sha256(layer).hexdigest()}" for layer in layers]
    config = distribution.canonical_json(
        {"architecture": platform.removeprefix("linux/"), "os": "linux"}
    )
    config_digest = f"sha256:{hashlib.sha256(config).hexdigest()}"
    manifest = distribution.canonical_json(
        {
            "config": {
                "digest": config_digest,
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "size": len(config),
            },
            "layers": [
                {
                    "digest": layer_digest,
                    "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                    "size": len(layer),
                }
                for layer, layer_digest in zip(layers, layer_digests, strict=True)
            ],
            "schemaVersion": 2,
        }
    )
    digest = f"sha256:{hashlib.sha256(manifest).hexdigest()}"
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
                    "size": len(manifest),
                }
            ],
            "schemaVersion": 2,
        }
    )
    with tarfile.open(path, "w") as archive:
        entries = [
            ("index.json", index),
            (f"blobs/sha256/{digest.removeprefix('sha256:')}", manifest),
            (f"blobs/sha256/{config_digest.removeprefix('sha256:')}", config),
            *[
                (f"blobs/sha256/{layer_digest.removeprefix('sha256:')}", layer)
                for layer, layer_digest in zip(layers, layer_digests, strict=True)
            ],
        ]
        for name, content in entries:
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return digest


def write_oci_layout(
    path: Path, platform: str, layer_members: list[tuple[str, bytes]]
) -> str:
    return write_oci_layout_layers(path, platform, [layer_members])


class PrnsdDistributionTests(unittest.TestCase):
    def test_dockerfile_frontend_has_deterministic_expose_history(self) -> None:
        syntax = (ROOT / "Dockerfile").read_text(encoding="utf-8").splitlines()[0]
        self.assertEqual(
            syntax,
            "# syntax=docker/dockerfile:1.26.0@sha256:"
            "ecfaec9ed6d810b56388c508f4121597bfbba70d41a6dfeee4d8cad5f295fc32",
        )

    def test_container_publishes_one_default_operator_context(self) -> None:
        dockerfile = (ROOT / "Dockerfile").read_text(encoding="utf-8")
        self.assertIn("--features tokio-cloud-host,observability,otlp", dockerfile)
        self.assertIn("ENV PRNSD_STATE_DIR=/var/lib/prnsd/.service\n", dockerfile)
        self.assertIn("EXPOSE 4242/tcp 4284/tcp\n", dockerfile)
        self.assertIn(
            'CMD ["/usr/local/bin/prnsd", "status", "--json"]', dockerfile
        )
        self.assertIn(
            'CMD ["run", "--service", "--config", "/var/lib/prnsd", '
            '"--persistence-policy", "required", "--bootstrap", "server", '
            '"--log-format", "json"]',
            dockerfile,
        )

    def test_native_archives_are_byte_reproducible_and_self_describing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "prnsd"
            binary.write_bytes(b"test daemon")
            source = root / "source.zip"
            source.write_bytes(b"exact source")
            source_checksum = root / "source.zip.sha256"
            source_checksum.write_text(
                f"{hashlib.sha256(source.read_bytes()).hexdigest()}  source.zip\n",
                encoding="utf-8",
            )
            first = root / "first" / f"prnsd-{VERSION}-x86_64-unknown-linux-gnu.tar.gz"
            second = root / "second" / first.name
            common = {
                "binary": binary,
                "target": "x86_64-unknown-linux-gnu",
                "source_commit": "a" * 40,
                "source_date_epoch": 1_785_330_739,
                "rustc": "rustc 1.96.0 (deadbeef 2026-01-01)",
                "source_archive": source,
                "source_checksum": source_checksum,
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
                        f"prnsd-{VERSION}-x86_64-unknown-linux-gnu/build-identity.json"
                    )
                )
            self.assertIn(
                f"prnsd-{VERSION}-x86_64-unknown-linux-gnu/THIRD_PARTY_NOTICES.md",
                names,
            )
            self.assertIn(
                f"prnsd-{VERSION}-x86_64-unknown-linux-gnu/source.zip",
                names,
            )
            self.assertEqual(identity["source_commit"], "a" * 40)
            self.assertEqual(
                identity["features"], ["tokio-host", "observability", "tray"]
            )
            self.assertEqual(
                identity["source_archive_sha256"],
                hashlib.sha256(source.read_bytes()).hexdigest(),
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

    def test_flasher_payload_assets_preserve_signed_board_identity(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            candidate = root / "candidate"
            assets = root / "assets"
            candidate.mkdir()
            assets.mkdir()
            targets = []
            for board, content in (
                ("heltec-v4", b"heltec application"),
                ("t-beam-supreme", b"t-beam application"),
            ):
                relative = f"firmware/hopspot/{board}/{VERSION}/application.bin"
                payload = candidate / relative
                payload.parent.mkdir(parents=True)
                payload.write_bytes(content)
                targets.append(
                    {
                        "board_slug": board,
                        "parts": [
                            {
                                "path": relative,
                                "sha256": hashlib.sha256(content).hexdigest(),
                                "size": len(content),
                            }
                        ],
                    }
                )
            (candidate / "flash-manifest.json").write_bytes(
                distribution.canonical_json(
                    {
                        "release": {"channel": "stable", "version": VERSION},
                        "targets": targets,
                    }
                )
            )
            distribution.stage_flasher_payloads(
                types.SimpleNamespace(candidate=candidate, assets=assets)
            )
            self.assertEqual(
                (assets / f"prns-hopspot-{VERSION}-heltec-v4-application.bin").read_bytes(),
                b"heltec application",
            )
            self.assertEqual(
                (
                    assets / f"prns-hopspot-{VERSION}-t-beam-supreme-application.bin"
                ).read_bytes(),
                b"t-beam application",
            )

            (candidate / targets[0]["parts"][0]["path"]).write_bytes(b"tampered")
            new_assets = root / "new-assets"
            new_assets.mkdir()
            with self.assertRaisesRegex(ValueError, "size differs"):
                distribution.stage_flasher_payloads(
                    types.SimpleNamespace(candidate=candidate, assets=new_assets)
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
            with self.assertRaisesRegex(
                ValueError, "exactly linux/amd64 and linux/arm64"
            ):
                distribution.write_image_metadata(arguments)

    def test_native_candidate_verification_rejects_post_index_changes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for target in distribution.TARGETS:
                (root / distribution.archive_name(VERSION, target)).write_bytes(
                    target.encode()
                )
                (root / f"{target}-linkage.txt").write_text(
                    "linkage\n", encoding="utf-8"
                )
            (root / f"prnsd-{VERSION}-source.spdx.json").write_text(
                "{}\n", encoding="utf-8"
            )
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
            source = b"exact source"
            root = Path(temporary) / "assets"
            root.mkdir()
            checksum = Path(temporary) / "source.zip.sha256"
            checksum.write_text(
                f"{hashlib.sha256(source).hexdigest()}  source.zip\n",
                encoding="utf-8",
            )
            for architecture in ("amd64", "arm64"):
                write_oci_layout(
                    root / f"prnsd-linux-{architecture}.oci.tar",
                    f"linux/{architecture}",
                    image_members(architecture, source),
                )
                (root / f"prnsd-linux-{architecture}.spdx.json").write_text(
                    "{}\n", encoding="utf-8"
                )
            index = root / f"prnsd-image-candidate-{'b' * 40}.json"
            arguments = types.SimpleNamespace(
                assets=root,
                source_commit="b" * 40,
                source_archive_checksum=checksum,
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

    def test_image_candidate_requires_the_commit_source_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            source = b"exact source"
            root = Path(temporary) / "assets"
            root.mkdir()
            checksum = Path(temporary) / "source.zip.sha256"
            checksum.write_text(
                f"{hashlib.sha256(source).hexdigest()}  source.zip\n",
                encoding="utf-8",
            )
            arguments = types.SimpleNamespace(
                assets=root,
                source_commit="b" * 40,
                source_archive_checksum=checksum,
                repository="KenAKAFrosty/Prns",
                workflow_run_id=52,
                workflow_run_attempt=1,
                output=root / f"prnsd-image-candidate-{'b' * 40}.json",
            )

            for architecture in ("amd64", "arm64"):
                write_oci_layout(
                    root / f"prnsd-linux-{architecture}.oci.tar",
                    f"linux/{architecture}",
                    [("usr/local/bin/prnsd", architecture.encode())],
                )
                (root / f"prnsd-linux-{architecture}.spdx.json").write_text(
                    "{}\n", encoding="utf-8"
                )
            with self.assertRaisesRegex(ValueError, "does not ship"):
                distribution.write_image_candidate_index(arguments)

            for architecture in ("amd64", "arm64"):
                write_oci_layout(
                    root / f"prnsd-linux-{architecture}.oci.tar",
                    f"linux/{architecture}",
                    image_members(architecture, b"stale source"),
                )
            with self.assertRaisesRegex(ValueError, "differs from the commit snapshot"):
                distribution.write_image_candidate_index(arguments)

            for architecture in ("amd64", "arm64"):
                write_oci_layout(
                    root / f"prnsd-linux-{architecture}.oci.tar",
                    f"linux/{architecture}",
                    image_members(architecture, source),
                )
            distribution.write_image_candidate_index(arguments)
            index = arguments.output
            value = json.loads(index.read_text(encoding="utf-8"))
            value["source_archive_sha256"] = "0" * 64
            index.write_bytes(distribution.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "recorded digest"):
                distribution.verify_image_candidate_index(
                    types.SimpleNamespace(
                        assets=root,
                        index=index,
                        source_commit="b" * 40,
                        repository="KenAKAFrosty/Prns",
                        workflow_run_id=52,
                    )
                )

    def test_image_source_parsing_applies_whiteouts_before_layer_additions(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            source = b"replacement source"
            layout = Path(temporary) / "image.oci.tar"
            write_oci_layout_layers(
                layout,
                "linux/amd64",
                [
                    image_members("amd64", b"lower source"),
                    [
                        *image_members("amd64", source)[1:],
                        ("usr/share/prnsd/.wh..wh..opq", b""),
                    ],
                ],
            )

            self.assertEqual(
                distribution.oci_source_archive_sha256(layout, "linux/amd64"),
                hashlib.sha256(source).hexdigest(),
            )

    def test_image_source_parsing_rejects_ambiguous_or_unsafe_layers(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = b"exact source"
            duplicate = root / "duplicate.oci.tar"
            members = image_members("amd64", source)
            write_oci_layout(
                duplicate,
                "linux/amd64",
                [*members, ("usr/share/prnsd/source.zip", source)],
            )
            with self.assertRaisesRegex(ValueError, "duplicate"):
                distribution.oci_source_archive_sha256(duplicate, "linux/amd64")

            unsafe = root / "unsafe.oci.tar"
            write_oci_layout(
                unsafe,
                "linux/amd64",
                [*members, ("../usr/share/prnsd/source.zip", source)],
            )
            with self.assertRaisesRegex(ValueError, "unsafe"):
                distribution.oci_source_archive_sha256(unsafe, "linux/amd64")

            removed = root / "removed.oci.tar"
            write_oci_layout_layers(
                removed,
                "linux/amd64",
                [members, [(".wh..wh..opq", b"")]],
            )
            with self.assertRaisesRegex(ValueError, "does not ship"):
                distribution.oci_source_archive_sha256(removed, "linux/amd64")

    def test_railway_contract_exposes_write_once_announcement_controls(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / f"railway-template-contract-v{VERSION}.json"
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
                controls["PRNSD_NNPAGES_ANNOUNCE"],
                {"allowed": ["Yes", "No"], "default": "Yes"},
            )
            self.assertEqual(
                controls["PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES"],
                {"default": "360", "unit": "minutes"},
            )

    def test_staging_railway_contract_is_explicit_and_digest_pinned(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commit = "c" * 40
            digest = f"sha256:{'d' * 64}"
            release_output = root / "release.json"
            staging_output = root / "staging.json"
            arguments = {
                "source_commit": commit,
                "image_digest": digest,
            }
            distribution.write_railway_contract(
                types.SimpleNamespace(output=release_output, **arguments)
            )
            distribution.write_staging_railway_contract(
                types.SimpleNamespace(output=staging_output, **arguments)
            )
            release = json.loads(release_output.read_text(encoding="utf-8"))
            staging = json.loads(staging_output.read_text(encoding="utf-8"))
            self.assertEqual(staging["channel"], "staging")
            self.assertEqual(
                staging["image"],
                f"ghcr.io/kenakafrosty/prnsd-staging@{digest}",
            )
            staging.pop("channel")
            staging["image"] = f"ghcr.io/kenakafrosty/prnsd@{digest}"
            self.assertEqual(staging, release)

    def test_staging_metadata_requires_verified_public_visibility(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commit = "c" * 40
            digest = f"sha256:{'d' * 64}"
            candidate = root / f"prnsd-image-candidate-{commit}.json"
            candidate.write_bytes(
                distribution.canonical_json(
                    {
                        "assets": [],
                        "platform_digests": {
                            "linux/amd64": f"sha256:{'e' * 64}",
                            "linux/arm64": f"sha256:{'f' * 64}",
                        },
                        "repository": "KenAKAFrosty/Prns",
                        "schema": 1,
                        "source_archive_sha256": "a" * 64,
                        "source_commit": commit,
                        "version": VERSION,
                        "workflow": {
                            "path": ".github/workflows/prnsd-image-candidate.yml",
                            "run_attempt": 2,
                            "run_id": 52,
                        },
                    }
                )
            )
            metadata = root / f"prnsd-staging-image-{commit}.json"
            arguments = types.SimpleNamespace(
                candidate_index=candidate,
                source_commit=commit,
                manifest_digest=digest,
                repository="KenAKAFrosty/Prns",
                image_candidate_run_id=52,
                workflow_run_id=61,
                workflow_run_attempt=3,
                visibility="public",
                output=metadata,
            )
            distribution.write_staging_metadata(arguments)
            verify = types.SimpleNamespace(
                metadata=metadata,
                source_commit=commit,
                image_digest=digest,
                repository="KenAKAFrosty/Prns",
                publication_run_id=61,
            )
            distribution.verify_staging_metadata(verify)
            value = json.loads(metadata.read_text(encoding="utf-8"))
            self.assertEqual(
                (value["channel"], value["image"], value["visibility"]),
                (
                    "staging",
                    "ghcr.io/kenakafrosty/prnsd-staging",
                    "public",
                ),
            )
            arguments.visibility = "private"
            distribution.write_staging_metadata(arguments)
            with self.assertRaisesRegex(ValueError, "public publication"):
                distribution.verify_staging_metadata(verify)

    def test_staging_deployment_evidence_cannot_verify_as_release_evidence(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release_output = root / f"deployment-qualification-v{VERSION}.json"
            common = {
                "source_commit": "c" * 40,
                "image_digest": f"sha256:{'d' * 64}",
                "repository": "KenAKAFrosty/Prns",
                "template_revision": "staging-2",
                "rollback_revision": "staging-1",
                "public_endpoint": "staging.example.com:4242",
                "identity_before": "e" * 32,
                "identity_after": "e" * 32,
                "observed_at": "2026-08-04T12:00:00Z",
                "workflow_run_id": 62,
                "workflow_run_attempt": 1,
            }
            distribution.write_deployment_evidence(
                types.SimpleNamespace(output=release_output, **common)
            )
            distribution.verify_deployment_evidence(
                types.SimpleNamespace(
                    evidence=release_output,
                    evidence_sha256=hashlib.sha256(
                        release_output.read_bytes()
                    ).hexdigest(),
                    source_commit="c" * 40,
                    image_digest=f"sha256:{'d' * 64}",
                    repository="KenAKAFrosty/Prns",
                    workflow_run_id=62,
                )
            )
            output = root / f"staging-deployment-{'c' * 40}.json"
            distribution.write_staging_deployment_evidence(
                types.SimpleNamespace(
                    publication_run_id=61,
                    output=output,
                    **common,
                )
            )
            value = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                (
                    value["channel"],
                    value["image"],
                    value["workflow"]["path"],
                ),
                (
                    "staging",
                    "ghcr.io/kenakafrosty/prnsd-staging",
                    ".github/workflows/prnsd-staging-qualification.yml",
                ),
            )
            with self.assertRaisesRegex(ValueError, "required release"):
                distribution.verify_deployment_evidence(
                    types.SimpleNamespace(
                        evidence=output,
                        evidence_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
                        source_commit="c" * 40,
                        image_digest=f"sha256:{'d' * 64}",
                        repository="KenAKAFrosty/Prns",
                        workflow_run_id=62,
                    )
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
                (assets / distribution.archive_name(VERSION, target)).write_bytes(
                    target.encode()
                )
                (assets / f"{target}-linkage.txt").write_text(
                    "linkage\n", encoding="utf-8"
                )
            for name in (
                f"prnsd-{VERSION}-source.spdx.json",
                "prnsd-linux-amd64.spdx.json",
                "prnsd-linux-arm64.spdx.json",
            ):
                (assets / name).write_text("{}\n", encoding="utf-8")
            for name in (
                f"prnsd-native-attestation-v{VERSION}.json",
                f"prnsd-image-attestation-v{VERSION}.json",
                f"prns-flasher-candidate-v{VERSION}-signed.tar.gz",
            ):
                (assets / name).write_text("evidence\n", encoding="utf-8")
            candidate = root / "flasher-candidate"
            candidate.mkdir()
            payload_content = b"signed firmware payload"
            payload_relative = f"firmware/hopspot/heltec-v4/{VERSION}/application.bin"
            payload = candidate / payload_relative
            payload.parent.mkdir(parents=True)
            payload.write_bytes(payload_content)
            manifest = {
                "release": {"channel": "stable", "version": VERSION},
                "targets": [
                    {
                        "board_slug": "heltec-v4",
                        "parts": [
                            {
                                "path": payload_relative,
                                "sha256": hashlib.sha256(payload_content).hexdigest(),
                                "size": len(payload_content),
                            }
                        ],
                    }
                ],
            }
            (candidate / "flash-manifest.json").write_bytes(
                distribution.canonical_json(manifest)
            )
            shutil.copy2(candidate / "flash-manifest.json", assets)
            distribution.stage_flasher_payloads(
                types.SimpleNamespace(candidate=candidate, assets=assets)
            )
            distribution.write_image_metadata(
                types.SimpleNamespace(
                    source_commit=commit,
                    manifest_digest=manifest_digest,
                    platform_digest=[
                        f"{platform}={digest}"
                        for platform, digest in platform_digests.items()
                    ],
                    output=assets / f"prnsd-image-v{VERSION}.json",
                )
            )
            distribution.write_railway_contract(
                types.SimpleNamespace(
                    source_commit=commit,
                    image_digest=manifest_digest,
                    output=assets / f"railway-template-contract-v{VERSION}.json",
                )
            )
            (assets / f"prnsd-candidate-{commit}.json").write_bytes(
                distribution.canonical_json(
                    {
                        "source_commit": commit,
                        "version": VERSION,
                        "workflow": {"path": ".github/workflows/prnsd-candidate.yml"},
                    }
                )
            )
            (assets / f"prnsd-image-candidate-{commit}.json").write_bytes(
                distribution.canonical_json(
                    {
                        "platform_digests": platform_digests,
                        "source_archive_sha256": "1" * 64,
                        "source_commit": commit,
                        "version": VERSION,
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
            record = custody / f"release-record-v{VERSION}.json"
            distribution.write_suite_record(
                types.SimpleNamespace(
                    assets=assets,
                    inventory=inventory,
                    source_commit=commit,
                    output=record,
                )
            )
            record_value = json.loads(record.read_text(encoding="utf-8"))
            self.assertEqual(
                record_value["flasher"]["payloads"],
                [
                    {
                        "asset": f"prns-hopspot-{VERSION}-heltec-v4-application.bin",
                        "board_slug": "heltec-v4",
                        "candidate_path": payload_relative,
                        "sha256": hashlib.sha256(payload_content).hexdigest(),
                        "size": len(payload_content),
                    }
                ],
            )
            for path in assets.iterdir():
                shutil.copy2(path, release / path.name)
            for path in (inventory, record):
                shutil.copy2(path, release / path.name)
                (release / f"{path.name}.minisig").write_text(
                    "signature\n", encoding="utf-8"
                )
            shutil.copy2(ROOT / "release/keys/minisign.pub", release / "minisign.pub")
            verify = types.SimpleNamespace(
                assets=release,
                source_commit=commit,
                image_digest=manifest_digest,
            )
            distribution.verify_suite_release(verify)
            (release / f"public-review-v{VERSION}-run-71-attempt-2.json").write_text(
                "{}\n", encoding="utf-8"
            )
            (release / f"qualification-evidence-v{VERSION}.tar.gz").write_bytes(
                b"separately signed flasher evidence"
            )
            (release / f"deployment-qualification-v{VERSION}.json").write_text(
                "{}\n", encoding="utf-8"
            )
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
