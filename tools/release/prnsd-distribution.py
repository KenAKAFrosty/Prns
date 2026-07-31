#!/usr/bin/env python3
"""Build and verify deterministic prnsd distribution evidence."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile
import tempfile
import types
import zipfile


ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
SHA256_PATTERN = re.compile(r"sha256:[0-9a-f]{64}\Z")
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}\Z")
PLATFORM_PATTERN = re.compile(r"linux/(?:amd64|arm64)\Z")
IDENTITY_HASH_PATTERN = re.compile(r"[0-9a-f]{32}\Z")
ARCHIVE_DIGEST_PATTERN = re.compile(r"[0-9a-f]{64}\Z")
SOURCE_CHECKSUM_PATTERN = re.compile(r"([0-9a-f]{64})  source\.zip\n\Z")
IMAGE_SOURCE_ARCHIVE_PATH = "usr/share/prnsd/source.zip"
IMAGE_SOURCE_CHECKSUM_PATH = "usr/share/prnsd/source.zip.sha256"
OCI_LAYER_MODES = {
    "application/vnd.oci.image.layer.v1.tar": "r:",
    "application/vnd.oci.image.layer.v1.tar+gzip": "r:gz",
}
MAX_IMAGE_SOURCE_ARCHIVE_BYTES = 32 * 1024 * 1024
IMAGE_SOURCE_PATHS = frozenset(
    {IMAGE_SOURCE_ARCHIVE_PATH, IMAGE_SOURCE_CHECKSUM_PATH}
)
IMAGE_SOURCE_ANCESTORS = frozenset(
    ancestor
    for hosted in IMAGE_SOURCE_PATHS
    for ancestor in (
        "/".join(PurePosixPath(hosted).parts[:depth])
        for depth in range(1, len(PurePosixPath(hosted).parts))
    )
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_json(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def suite_version() -> str:
    version = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("VERSION must contain one three-part release version")
    return version


def require_commit(value: str) -> str:
    if COMMIT_PATTERN.fullmatch(value) is None:
        raise ValueError("source commit must be one lowercase full Git commit")
    return value


def require_epoch(value: int) -> int:
    if value < 315532800:
        raise ValueError("source-date-epoch must be on or after 1980-01-01")
    return value


def archive_name(version: str, target: str) -> str:
    return f"prnsd-{version}-{target}{TARGETS[target]}"


def regular_file(path: Path, label: str) -> Path:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} must be one regular file: {path}")
    return path


def archive_members(
    *,
    binary: Path,
    target: str,
    version: str,
    commit: str,
    epoch: int,
    rustc: str,
    source_archive: Path,
    source_checksum: Path,
) -> tuple[str, list[tuple[str, bytes, int]]]:
    root = f"prnsd-{version}-{target}"
    executable = "prnsd.exe" if target.endswith("-windows-msvc") else "prnsd"
    binary_bytes = regular_file(binary, "daemon binary").read_bytes()
    if not binary_bytes:
        raise ValueError("daemon binary is empty")
    source_bytes = regular_file(source_archive, "source archive").read_bytes()
    source_checksum_bytes = regular_file(
        source_checksum, "source archive checksum"
    ).read_bytes()
    expected_source_checksum = (
        f"{hashlib.sha256(source_bytes).hexdigest()}  source.zip\n".encode()
    )
    if source_checksum_bytes != expected_source_checksum:
        raise ValueError("source archive checksum is malformed or stale")
    identity = {
        "binary_sha256": hashlib.sha256(binary_bytes).hexdigest(),
        "features": ["tokio-host", "observability", "tray"],
        "profile": "release",
        "rustc": rustc.strip(),
        "schema": 1,
        "source_commit": commit,
        "source_archive_sha256": hashlib.sha256(source_bytes).hexdigest(),
        "source_date_epoch": epoch,
        "target": target,
        "version": version,
    }
    sources = (
        (executable, binary_bytes, 0o755),
        ("LICENSE-APACHE", regular_file(ROOT / "LICENSE-APACHE", "Apache license").read_bytes(), 0o644),
        ("LICENSE-MIT", regular_file(ROOT / "LICENSE-MIT", "MIT license").read_bytes(), 0o644),
        (
            "THIRD_PARTY_NOTICES.md",
            regular_file(ROOT / "THIRD_PARTY_NOTICES.md", "third-party notices").read_bytes(),
            0o644,
        ),
        (
            "minisign.pub",
            regular_file(ROOT / "release/keys/minisign.pub", "Minisign public key").read_bytes(),
            0o644,
        ),
        ("build-identity.json", canonical_json(identity), 0o644),
        ("source.zip", source_bytes, 0o644),
        ("source.zip.sha256", source_checksum_bytes, 0o644),
    )
    return root, [(f"{root}/{name}", content, mode) for name, content, mode in sources]


def tar_archive(output: Path, root: str, members: list[tuple[str, bytes, int]], epoch: int) -> None:
    with output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                directory = tarfile.TarInfo(root)
                directory.type = tarfile.DIRTYPE
                directory.mode = 0o755
                directory.uid = directory.gid = 0
                directory.uname = directory.gname = ""
                directory.mtime = epoch
                archive.addfile(directory)
                for name, content, mode in members:
                    info = tarfile.TarInfo(name)
                    info.size = len(content)
                    info.mode = mode
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = epoch
                    archive.addfile(info, io.BytesIO(content))


def zip_archive(output: Path, members: list[tuple[str, bytes, int]], epoch: int) -> None:
    import datetime

    timestamp = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc)
    date_time = (
        timestamp.year,
        timestamp.month,
        timestamp.day,
        timestamp.hour,
        timestamp.minute,
        timestamp.second - timestamp.second % 2,
    )
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, content, mode in members:
            info = zipfile.ZipInfo(name, date_time)
            info.create_system = 3
            info.external_attr = (mode & 0xFFFF) << 16
            info.compress_type = zipfile.ZIP_STORED
            archive.writestr(info, content)


def build_archive(arguments: argparse.Namespace) -> None:
    version = suite_version()
    target = arguments.target
    expected_name = archive_name(version, target)
    if arguments.output.name != expected_name:
        raise ValueError(f"archive output must be named {expected_name}")
    commit = require_commit(arguments.source_commit)
    epoch = require_epoch(arguments.source_date_epoch)
    root, members = archive_members(
        binary=arguments.binary,
        target=target,
        version=version,
        commit=commit,
        epoch=epoch,
        rustc=arguments.rustc,
        source_archive=arguments.source_archive,
        source_checksum=arguments.source_checksum,
    )
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="prnsd-archive-", dir=arguments.output.parent
    ) as temporary:
        staged = Path(temporary) / expected_name
        if TARGETS[target] == ".zip":
            zip_archive(staged, members, epoch)
        else:
            tar_archive(staged, root, members, epoch)
        os.replace(staged, arguments.output)
    print(f"created {arguments.output} ({sha256(arguments.output)})")


def compare_files(arguments: argparse.Namespace) -> None:
    primary = regular_file(arguments.primary, "primary artifact")
    reproduction = regular_file(arguments.reproduction, "reproduction artifact")
    if primary.read_bytes() != reproduction.read_bytes():
        raise ValueError(
            "reproduction bytes differ: "
            f"{primary.name}={sha256(primary)} {reproduction.name}={sha256(reproduction)}"
        )
    print(f"reproduced {primary.name} byte for byte ({sha256(primary)})")


def parse_platform_digest(value: str) -> tuple[str, str]:
    try:
        platform, digest = value.split("=", maxsplit=1)
    except ValueError as error:
        raise ValueError("platform digest must use PLATFORM=sha256:DIGEST") from error
    if PLATFORM_PATTERN.fullmatch(platform) is None or SHA256_PATTERN.fullmatch(digest) is None:
        raise ValueError(f"invalid platform digest {value!r}")
    return platform, digest


def safe_oci_members(archive: tarfile.TarFile) -> dict[str, tarfile.TarInfo]:
    listed = archive.getmembers()
    if any(
        member.issym()
        or member.islnk()
        or PurePosixPath(member.name).is_absolute()
        or ".." in PurePosixPath(member.name).parts
        for member in listed
    ):
        raise ValueError("OCI layout contains an unsafe archive member")
    members = {member.name: member for member in listed}
    if len(members) != len(listed):
        raise ValueError("OCI layout contains duplicate archive members")
    return members


def oci_blob(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    digest: str,
    label: str,
) -> bytes:
    blob = members.get(f"blobs/sha256/{digest.removeprefix('sha256:')}")
    if blob is None or not blob.isfile():
        raise ValueError(f"OCI {label} blob is missing")
    extracted = archive.extractfile(blob)
    if extracted is None:
        raise ValueError(f"OCI {label} blob could not be read")
    content = extracted.read()
    if digest != f"sha256:{hashlib.sha256(content).hexdigest()}":
        raise ValueError(f"OCI {label} digest does not match its bytes")
    return content


def platform_manifest_digest(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    platform: str,
) -> str:
    index_member = members.get("index.json")
    if index_member is None or not index_member.isfile():
        raise ValueError("OCI layout has no regular index.json")
    extracted = archive.extractfile(index_member)
    if extracted is None:
        raise ValueError("OCI layout index could not be read")
    index = json.load(extracted)
    manifests = index.get("manifests") if isinstance(index, dict) else None
    if not isinstance(manifests, list):
        raise ValueError("OCI layout index has no manifest list")
    architecture = platform.removeprefix("linux/")
    matches = [
        descriptor
        for descriptor in manifests
        if isinstance(descriptor, dict)
        and isinstance(descriptor.get("platform"), dict)
        and descriptor.get("platform", {}).get("os") == "linux"
        and descriptor.get("platform", {}).get("architecture") == architecture
        and SHA256_PATTERN.fullmatch(str(descriptor.get("digest"))) is not None
    ]
    if len(matches) != 1:
        raise ValueError(f"OCI layout does not contain exactly one {platform} image")
    return matches[0]["digest"]


def oci_platform_digest(layout: Path, platform: str) -> str:
    if PLATFORM_PATTERN.fullmatch(platform) is None:
        raise ValueError(f"unsupported OCI platform {platform!r}")
    regular_file(layout, "OCI layout archive")
    with tarfile.open(layout, "r:*") as archive:
        members = safe_oci_members(archive)
        digest = platform_manifest_digest(archive, members, platform)
        oci_blob(archive, members, digest, "platform manifest")
        return digest


def oci_source_archive_sha256(layout: Path, platform: str) -> str:
    if PLATFORM_PATTERN.fullmatch(platform) is None:
        raise ValueError(f"unsupported OCI platform {platform!r}")
    regular_file(layout, "OCI layout archive")
    with tarfile.open(layout, "r:*") as archive:
        members = safe_oci_members(archive)
        digest = platform_manifest_digest(archive, members, platform)
        manifest = json.loads(oci_blob(archive, members, digest, "platform manifest"))
        layers = manifest.get("layers") if isinstance(manifest, dict) else None
        if not isinstance(layers, list) or not layers:
            raise ValueError(f"{platform} manifest has no layer list")
        hosted: dict[str, bytes | None] = dict.fromkeys(IMAGE_SOURCE_PATHS)
        for descriptor in layers:
            if not isinstance(descriptor, dict):
                raise ValueError(f"{platform} manifest layer descriptor is malformed")
            mode = OCI_LAYER_MODES.get(str(descriptor.get("mediaType")))
            layer_digest = str(descriptor.get("digest"))
            if mode is None or SHA256_PATTERN.fullmatch(layer_digest) is None:
                raise ValueError(f"{platform} manifest layer descriptor is malformed")
            layer_bytes = oci_blob(archive, members, layer_digest, "layer")
            with tarfile.open(fileobj=io.BytesIO(layer_bytes), mode=mode) as layer:
                additions: dict[str, bytes] = {}
                whiteouts: list[tuple[str, str]] = []
                relevant_names: set[str] = set()
                for member in layer.getmembers():
                    name = member.name
                    while name.startswith("./"):
                        name = name[2:]
                    path = PurePosixPath(name)
                    if path.is_absolute() or ".." in path.parts:
                        raise ValueError(
                            f"{platform} image layer contains an unsafe archive member"
                        )
                    name = path.as_posix()
                    directory, _, base = name.rpartition("/")
                    if base == ".wh..wh..opq":
                        effect = ("opaque", directory)
                    elif base.startswith(".wh."):
                        removed_base = base.removeprefix(".wh.")
                        if not removed_base:
                            raise ValueError(
                                f"{platform} image layer contains an invalid whiteout"
                            )
                        removed = (
                            f"{directory}/{removed_base}"
                            if directory
                            else removed_base
                        )
                        effect = ("remove", removed)
                    else:
                        effect = None
                    if effect is None:
                        affects_source = False
                    elif effect[0] == "opaque":
                        affects_source = not effect[1] or any(
                            hosted_path.startswith(f"{effect[1]}/")
                            for hosted_path in IMAGE_SOURCE_PATHS
                        )
                    else:
                        affects_source = any(
                            hosted_path == effect[1]
                            or hosted_path.startswith(f"{effect[1]}/")
                            for hosted_path in IMAGE_SOURCE_PATHS
                        )
                    relevant = (
                        name in IMAGE_SOURCE_PATHS
                        or name in IMAGE_SOURCE_ANCESTORS
                        or affects_source
                    )
                    if relevant:
                        if name in relevant_names:
                            raise ValueError(
                                f"{platform} image layer contains duplicate {name}"
                            )
                        relevant_names.add(name)
                    if effect is not None:
                        if affects_source:
                            whiteouts.append(effect)
                        continue
                    if name in IMAGE_SOURCE_ANCESTORS:
                        if not member.isdir():
                            raise ValueError(
                                f"{platform} image hosts an invalid ancestor {name}"
                            )
                        continue
                    if name not in IMAGE_SOURCE_PATHS:
                        continue
                    if not member.isfile() or member.size > MAX_IMAGE_SOURCE_ARCHIVE_BYTES:
                        raise ValueError(f"{platform} image hosts an invalid {name}")
                    extracted = layer.extractfile(member)
                    if extracted is None:
                        raise ValueError(f"{platform} image {name} could not be read")
                    additions[name] = extracted.read()
                for effect, affected in whiteouts:
                    for hosted_path in IMAGE_SOURCE_PATHS:
                        if effect == "opaque":
                            removes = bool(affected) and hosted_path.startswith(
                                f"{affected}/"
                            )
                            if not affected:
                                removes = True
                        else:
                            removes = hosted_path == affected or hosted_path.startswith(
                                f"{affected}/"
                            )
                        if removes:
                            hosted[hosted_path] = None
                hosted.update(additions)
        source = hosted[IMAGE_SOURCE_ARCHIVE_PATH]
        checksum = hosted[IMAGE_SOURCE_CHECKSUM_PATH]
        if source is None or checksum is None:
            raise ValueError(
                f"{platform} image does not ship {IMAGE_SOURCE_ARCHIVE_PATH}"
            )
        actual = hashlib.sha256(source).hexdigest()
        if checksum != f"{actual}  source.zip\n".encode():
            raise ValueError(f"{platform} image source checksum is malformed or stale")
        return actual


def parse_source_checksum(path: Path) -> str:
    document = regular_file(path, "source archive checksum").read_text(encoding="utf-8")
    match = SOURCE_CHECKSUM_PATTERN.fullmatch(document)
    if match is None:
        raise ValueError("source archive checksum sidecar is malformed")
    return match.group(1)


def print_oci_digest(arguments: argparse.Namespace) -> None:
    print(oci_platform_digest(arguments.layout, arguments.platform))


def compare_oci(arguments: argparse.Namespace) -> None:
    primary = oci_platform_digest(arguments.primary, arguments.platform)
    reproduction = oci_platform_digest(arguments.reproduction, arguments.platform)
    if primary != reproduction:
        raise ValueError(
            f"{arguments.platform} OCI reproduction differs: "
            f"primary={primary} reproduction={reproduction}"
        )
    print(f"reproduced {arguments.platform} OCI manifest ({primary})")


def write_image_metadata(arguments: argparse.Namespace) -> None:
    version = suite_version()
    commit = require_commit(arguments.source_commit)
    manifest = arguments.manifest_digest
    if SHA256_PATTERN.fullmatch(manifest) is None:
        raise ValueError("manifest digest must be one lowercase SHA-256 OCI digest")
    platforms = dict(parse_platform_digest(value) for value in arguments.platform_digest)
    if set(platforms) != {"linux/amd64", "linux/arm64"}:
        raise ValueError("image metadata requires exactly linux/amd64 and linux/arm64")
    value = {
        "candidate": f"ghcr.io/kenakafrosty/prnsd:candidate-{commit}",
        "image": "ghcr.io/kenakafrosty/prnsd",
        "manifest_digest": manifest,
        "platform_digests": platforms,
        "schema": 1,
        "source_commit": commit,
        "version": version,
    }
    arguments.output.write_bytes(canonical_json(value))
    print(f"wrote {arguments.output} ({sha256(arguments.output)})")


def write_railway_contract(arguments: argparse.Namespace) -> None:
    commit = require_commit(arguments.source_commit)
    if SHA256_PATTERN.fullmatch(arguments.image_digest) is None:
        raise ValueError("Railway image digest must be one OCI SHA-256 digest")
    value = {
        "bootstrap": {
            "operator_environment": {
                "PRNSD_BACKBONE_DISCOVERABLE": {
                    "allowed": ["Yes", "No"],
                    "default": "Yes",
                },
                "PRNSD_NNPAGES_ANNOUNCE": {
                    "allowed": ["Yes", "No"],
                    "default": "Yes",
                },
                "PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES": {
                    "default": "360",
                    "unit": "minutes",
                },
            },
            "write_once": True,
        },
        "healthcheck": {
            "http_path": None,
            "readiness_command": "prnsd status --config /var/lib/prnsd --json",
        },
        "image": f"ghcr.io/kenakafrosty/prnsd@{arguments.image_digest}",
        "logging": "json",
        "network": {
            "internal_port": 4242,
            "kind": "tcp_proxy",
            "published_endpoint_environment": [
                "RAILWAY_TCP_PROXY_DOMAIN",
                "RAILWAY_TCP_PROXY_PORT",
            ],
        },
        "replicas": 1,
        "restart_policy": "on_failure",
        "schema": 1,
        "source_commit": commit,
        "version": suite_version(),
        "volume": {
            "mount_path": "/var/lib/prnsd",
            "required": True,
        },
    }
    arguments.output.write_bytes(canonical_json(value))
    print(f"wrote digest-pinned Railway publication contract {arguments.output}")


def file_identity(path: Path) -> dict[str, str | int]:
    regular_file(path, f"release evidence {path.name}")
    return {"name": path.name, "sha256": sha256(path), "size": path.stat().st_size}


def write_candidate_index(arguments: argparse.Namespace) -> None:
    root = arguments.assets.resolve()
    version = suite_version()
    commit = require_commit(arguments.source_commit)
    if arguments.workflow_run_id <= 0 or arguments.workflow_run_attempt <= 0:
        raise ValueError("workflow run identity must be positive")
    expected_archives = {archive_name(version, target) for target in TARGETS}
    actual_archives = {
        path.name
        for path in root.iterdir()
        if path.name.startswith(f"prnsd-{version}-")
        and (path.name.endswith(".tar.gz") or path.name.endswith(".zip"))
    }
    if actual_archives != expected_archives:
        raise ValueError(
            "daemon candidate has the wrong native archive matrix: "
            f"missing={sorted(expected_archives - actual_archives)}, "
            f"unexpected={sorted(actual_archives - expected_archives)}"
        )
    files = [
        file_identity(path)
        for path in sorted(root.iterdir(), key=lambda path: path.name)
        if path.resolve() != arguments.output.resolve()
    ]
    if not any(str(item["name"]).endswith(".spdx.json") for item in files):
        raise ValueError("daemon candidate has no SPDX SBOM")
    if len([item for item in files if str(item["name"]).endswith("-linkage.txt")]) != 5:
        raise ValueError("daemon candidate requires linkage evidence for all five targets")
    value = {
        "assets": files,
        "repository": arguments.repository,
        "schema": 1,
        "source_commit": commit,
        "version": version,
        "workflow": {
            "path": ".github/workflows/prnsd-candidate.yml",
            "run_attempt": arguments.workflow_run_attempt,
            "run_id": arguments.workflow_run_id,
        },
    }
    arguments.output.write_bytes(canonical_json(value))
    print(f"wrote exact daemon candidate index {arguments.output}")


def write_image_candidate_index(arguments: argparse.Namespace) -> None:
    root = arguments.assets.resolve()
    commit = require_commit(arguments.source_commit)
    if arguments.workflow_run_id <= 0 or arguments.workflow_run_attempt <= 0:
        raise ValueError("workflow run identity must be positive")
    layouts = {
        "linux/amd64": root / "prnsd-linux-amd64.oci.tar",
        "linux/arm64": root / "prnsd-linux-arm64.oci.tar",
    }
    platform_digests = {
        platform: oci_platform_digest(path, platform)
        for platform, path in layouts.items()
    }
    expected_source = parse_source_checksum(arguments.source_archive_checksum)
    for platform, path in layouts.items():
        if oci_source_archive_sha256(path, platform) != expected_source:
            raise ValueError(
                f"{platform} image source archive differs from the commit snapshot"
            )
    sboms = sorted(root.glob("prnsd-linux-*.spdx.json"))
    if len(sboms) != 2:
        raise ValueError("image candidate requires one SPDX SBOM per platform")
    files = [
        file_identity(path)
        for path in sorted([*layouts.values(), *sboms], key=lambda path: path.name)
    ]
    value = {
        "assets": files,
        "platform_digests": platform_digests,
        "repository": arguments.repository,
        "schema": 1,
        "source_archive_sha256": expected_source,
        "source_commit": commit,
        "version": suite_version(),
        "workflow": {
            "path": ".github/workflows/prnsd-image-candidate.yml",
            "run_attempt": arguments.workflow_run_attempt,
            "run_id": arguments.workflow_run_id,
        },
    }
    arguments.output.write_bytes(canonical_json(value))
    print(f"wrote exact image candidate index {arguments.output}")


def indexed_files(root: Path, index: Path) -> list[dict[str, str | int]]:
    if not root.is_dir() or root.is_symlink():
        raise ValueError("candidate assets must be one regular directory")
    files: list[Path] = []
    for path in root.iterdir():
        if path.resolve() == index.resolve():
            continue
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"candidate assets must be flat regular files: {path}")
        files.append(path)
    return [
        file_identity(path) for path in sorted(files, key=lambda candidate: candidate.name)
    ]


def load_candidate_index(
    arguments: argparse.Namespace, workflow_path: str
) -> tuple[Path, dict]:
    commit = require_commit(arguments.source_commit)
    if arguments.workflow_run_id <= 0:
        raise ValueError("candidate workflow run identity must be positive")
    index = regular_file(arguments.index, "candidate index")
    value = json.loads(index.read_text(encoding="utf-8"))
    required_fields = {
        "assets",
        "repository",
        "schema",
        "source_commit",
        "version",
        "workflow",
    }
    if "image" in workflow_path:
        required_fields.update({"platform_digests", "source_archive_sha256"})
    workflow = value.get("workflow") if isinstance(value, dict) else None
    if (
        not isinstance(value, dict)
        or set(value) != required_fields
        or value.get("schema") != 1
        or value.get("version") != suite_version()
        or value.get("source_commit") != commit
        or value.get("repository") != arguments.repository
        or not isinstance(workflow, dict)
        or set(workflow) != {"path", "run_attempt", "run_id"}
        or workflow.get("path") != workflow_path
        or workflow.get("run_id") != arguments.workflow_run_id
        or not isinstance(workflow.get("run_attempt"), int)
        or isinstance(workflow.get("run_attempt"), bool)
        or workflow["run_attempt"] <= 0
    ):
        raise ValueError("candidate index differs from the required producer identity")
    expected_name = (
        f"prnsd-image-candidate-{commit}.json"
        if "image" in workflow_path
        else f"prnsd-candidate-{commit}.json"
    )
    if index.name != expected_name:
        raise ValueError(f"candidate index must be named {expected_name}")
    return index, value


def verify_candidate_index(arguments: argparse.Namespace) -> None:
    index, value = load_candidate_index(
        arguments, ".github/workflows/prnsd-candidate.yml"
    )
    root = arguments.assets.resolve()
    actual = indexed_files(root, index)
    if value.get("assets") != actual:
        raise ValueError("native candidate files differ from their producer index")
    names = {str(item["name"]) for item in actual}
    version = suite_version()
    expected_archives = {archive_name(version, target) for target in TARGETS}
    if not expected_archives.issubset(names):
        raise ValueError("native candidate index is missing the exact archive matrix")
    if len([name for name in names if name.endswith("-linkage.txt")]) != 5:
        raise ValueError("native candidate index is missing linkage evidence")
    if f"prnsd-{version}-source.spdx.json" not in names:
        raise ValueError("native candidate index is missing its source SPDX SBOM")
    print(f"verified exact native candidate from workflow run {arguments.workflow_run_id}")


def verify_image_candidate_index(arguments: argparse.Namespace) -> None:
    index, value = load_candidate_index(
        arguments, ".github/workflows/prnsd-image-candidate.yml"
    )
    required_fields = {
        "assets",
        "platform_digests",
        "repository",
        "schema",
        "source_archive_sha256",
        "source_commit",
        "version",
        "workflow",
    }
    if set(value) != required_fields:
        raise ValueError("image candidate index has an unsupported shape")
    root = arguments.assets.resolve()
    actual = indexed_files(root, index)
    if value.get("assets") != actual:
        raise ValueError("image candidate files differ from their producer index")
    expected_names = {
        "prnsd-linux-amd64.oci.tar",
        "prnsd-linux-arm64.oci.tar",
        "prnsd-linux-amd64.spdx.json",
        "prnsd-linux-arm64.spdx.json",
    }
    if {str(item["name"]) for item in actual} != expected_names:
        raise ValueError("image candidate does not contain its exact OCI and SBOM matrix")
    expected_digests = {
        platform: oci_platform_digest(
            root / f"prnsd-{platform.replace('/', '-')}.oci.tar", platform
        )
        for platform in ("linux/amd64", "linux/arm64")
    }
    if value.get("platform_digests") != expected_digests:
        raise ValueError("image candidate platform digests differ from its OCI layouts")
    recorded_source = value.get("source_archive_sha256")
    if (
        not isinstance(recorded_source, str)
        or ARCHIVE_DIGEST_PATTERN.fullmatch(recorded_source) is None
    ):
        raise ValueError("image candidate records an invalid source archive digest")
    for platform in ("linux/amd64", "linux/arm64"):
        actual_source = oci_source_archive_sha256(
            root / f"prnsd-{platform.replace('/', '-')}.oci.tar", platform
        )
        if actual_source != recorded_source:
            raise ValueError(
                "image candidate source archive differs from its recorded digest"
            )
    print(f"verified exact image candidate from workflow run {arguments.workflow_run_id}")


def create_inventory(arguments: argparse.Namespace) -> None:
    root = arguments.assets.resolve()
    if not root.is_dir() or root.is_symlink():
        raise ValueError("assets directory must be one regular directory")
    output = arguments.output.resolve()
    excluded = {output.name, f"{output.name}.minisig"}
    entries: list[Path] = []
    for entry in root.iterdir():
        if entry.name in excluded:
            continue
        if not entry.is_file() or entry.is_symlink():
            raise ValueError(f"release assets must be flat regular files: {entry}")
        if "\n" in entry.name or "\r" in entry.name or "  " in entry.name:
            raise ValueError(f"release asset has an unsafe checksum name: {entry.name!r}")
        entries.append(entry)
    if not entries:
        raise ValueError("release asset inventory cannot be empty")
    lines = [f"{sha256(path)}  {path.name}\n" for path in sorted(entries, key=lambda path: path.name)]
    output.write_text("".join(lines), encoding="utf-8", newline="\n")
    print(f"inventoried {len(entries)} release assets in {output}")


def read_inventory(inventory: Path) -> dict[str, str]:
    expected: dict[str, str] = {}
    for line in inventory.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^/\\\r\n]+)", line)
        if match is None or match.group(2) in expected:
            raise ValueError("checksum inventory is malformed or ambiguous")
        expected[match.group(2)] = match.group(1)
    if not expected:
        raise ValueError("checksum inventory cannot be empty")
    return expected


def verify_inventory(arguments: argparse.Namespace) -> None:
    root = arguments.assets.resolve()
    inventory = regular_file(arguments.inventory, "checksum inventory")
    expected = read_inventory(inventory)
    actual = {
        path.name: path
        for path in root.iterdir()
        if path.name
        not in {inventory.name, f"{inventory.name}.minisig"}
    }
    if set(actual) != set(expected):
        raise ValueError(
            "asset inventory differs from the directory: "
            f"missing={sorted(set(expected) - set(actual))}, "
            f"unexpected={sorted(set(actual) - set(expected))}"
        )
    for name, checksum in expected.items():
        regular_file(actual[name], f"release asset {name}")
        if sha256(actual[name]) != checksum:
            raise ValueError(f"release asset checksum differs: {name}")
    print(f"verified {len(expected)} exact release assets")


def suite_record_value(root: Path, inventory: Path, commit: str) -> dict:
    version = suite_version()
    commit = require_commit(commit)
    root = root.resolve()
    inventory = regular_file(inventory, "suite checksum inventory")
    expected = read_inventory(inventory)
    daemon_names = {archive_name(version, target) for target in TARGETS}
    if not daemon_names.issubset(expected):
        raise ValueError("suite inventory is missing one or more native daemon archives")
    flasher_bundle = f"prns-flasher-candidate-v{version}-signed.tar.gz"
    if flasher_bundle not in expected:
        raise ValueError("suite inventory is missing the signed flasher candidate")
    image_metadata_name = f"prnsd-image-v{version}.json"
    image_metadata_path = regular_file(
        root / image_metadata_name, "signed-image metadata"
    )
    metadata = json.loads(image_metadata_path.read_text(encoding="utf-8"))
    if (
        not isinstance(metadata, dict)
        or metadata.get("version") != version
        or metadata.get("source_commit") != commit
        or SHA256_PATTERN.fullmatch(str(metadata.get("manifest_digest"))) is None
    ):
        raise ValueError("image metadata differs from the suite release identity")
    image_candidate_name = f"prnsd-image-candidate-{commit}.json"
    native_candidate_name = f"prnsd-candidate-{commit}.json"
    for name, workflow_path in (
        (native_candidate_name, ".github/workflows/prnsd-candidate.yml"),
        (image_candidate_name, ".github/workflows/prnsd-image-candidate.yml"),
    ):
        if name not in expected:
            raise ValueError(f"suite inventory is missing producer index {name}")
        candidate = json.loads(
            regular_file(root / name, f"suite producer index {name}").read_text(
                encoding="utf-8"
            )
        )
        workflow = candidate.get("workflow") if isinstance(candidate, dict) else None
        if (
            not isinstance(candidate, dict)
            or candidate.get("version") != version
            or candidate.get("source_commit") != commit
            or not isinstance(workflow, dict)
            or workflow.get("path") != workflow_path
        ):
            raise ValueError(f"suite producer index {name} has the wrong identity")
    image_candidate = json.loads(
        (root / image_candidate_name).read_text(encoding="utf-8")
    )
    source_archive_sha256 = (
        image_candidate.get("source_archive_sha256")
        if isinstance(image_candidate, dict)
        else None
    )
    if (
        not isinstance(image_candidate, dict)
        or image_candidate.get("platform_digests") != metadata.get("platform_digests")
        or not isinstance(source_archive_sha256, str)
        or ARCHIVE_DIGEST_PATTERN.fullmatch(source_archive_sha256) is None
    ):
        raise ValueError("image candidate differs from the reproduced source identity")
    railway_name = f"railway-template-contract-v{version}.json"
    if railway_name not in expected:
        raise ValueError("suite inventory is missing the Railway publication contract")
    railway = json.loads((root / railway_name).read_text(encoding="utf-8"))
    if (
        not isinstance(railway, dict)
        or railway.get("schema") != 1
        or railway.get("version") != version
        or railway.get("source_commit") != commit
        or railway.get("image")
        != f"ghcr.io/kenakafrosty/prnsd@{metadata['manifest_digest']}"
        or railway.get("replicas") != 1
        or railway.get("volume")
        != {"mount_path": "/var/lib/prnsd", "required": True}
        or not isinstance(railway.get("network"), dict)
        or railway["network"].get("internal_port") != 4242
    ):
        raise ValueError("Railway contract differs from the signed image identity")
    linkage = sorted(name for name in expected if name.endswith("-linkage.txt"))
    if len(linkage) != 5:
        raise ValueError("suite inventory requires linkage evidence for all native targets")
    sboms = sorted(name for name in expected if name.endswith(".spdx.json"))
    if len(sboms) < 3:
        raise ValueError("suite inventory requires source and per-platform SPDX SBOMs")
    attestations = sorted(name for name in expected if "attestation" in name)
    if len(attestations) < 2:
        raise ValueError("suite inventory requires native and OCI provenance attestations")
    assets = [
        {
            "name": name,
            "sha256": checksum,
            "size": regular_file(root / name, f"suite asset {name}").stat().st_size,
        }
        for name, checksum in sorted(expected.items())
    ]
    return {
        "assets": assets,
        "attestations": attestations,
        "daemon_archives": sorted(daemon_names),
        "flasher": {
            "signed_candidate": flasher_bundle,
        },
        "image": {
            "metadata": image_metadata_name,
            "manifest_digest": metadata["manifest_digest"],
            "platform_digests": metadata.get("platform_digests"),
            "source_archive_sha256": source_archive_sha256,
        },
        "inventory": file_identity(inventory),
        "linkage": linkage,
        "railway": {
            "contract": railway_name,
        },
        "release": {
            "source_commit": commit,
            "version": version,
        },
        "sboms": sboms,
        "schema": 2,
        "trust_root": file_identity(ROOT / "release/keys/minisign.pub"),
    }


def write_suite_record(arguments: argparse.Namespace) -> None:
    verify_inventory(
        types.SimpleNamespace(assets=arguments.assets, inventory=arguments.inventory)
    )
    value = suite_record_value(
        arguments.assets, arguments.inventory, arguments.source_commit
    )
    arguments.output.write_bytes(canonical_json(value))
    print(f"wrote suite release record {arguments.output}")


def verify_suite_release(arguments: argparse.Namespace) -> None:
    version = suite_version()
    commit = require_commit(arguments.source_commit)
    if SHA256_PATTERN.fullmatch(arguments.image_digest) is None:
        raise ValueError("required image digest must be one OCI SHA-256 digest")
    root = arguments.assets.resolve()
    inventory = regular_file(root / "SHA256SUMS.txt", "suite checksum inventory")
    inventory_signature = regular_file(
        root / "SHA256SUMS.txt.minisig", "suite checksum signature"
    )
    record_path = regular_file(
        root / f"release-record-v{version}.json", "suite release record"
    )
    record_signature = regular_file(
        root / f"release-record-v{version}.json.minisig", "suite release record signature"
    )
    public_key = regular_file(root / "minisign.pub", "suite Minisign public key")
    if public_key.read_bytes() != (ROOT / "release/keys/minisign.pub").read_bytes():
        raise ValueError("release public key differs from the repository trust root")
    expected = read_inventory(inventory)
    custody = {
        inventory.name,
        inventory_signature.name,
        record_path.name,
        record_signature.name,
        public_key.name,
    }
    actual: dict[str, Path] = {}
    for path in root.iterdir():
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"downloaded suite contains a non-regular asset: {path.name}")
        actual[path.name] = path
    supplemental = {
        name
        for name in actual
        if name
        in {
            f"acceptance-v{version}.json",
            f"acceptance-v{version}.json.minisig",
            f"flasher-release-record-v{version}.json",
            f"flasher-release-record-v{version}.json.minisig",
            f"deployment-qualification-v{version}.json",
            f"qualification-evidence-v{version}.tar.gz",
        }
        or re.fullmatch(
            rf"public-review-v{re.escape(version)}-run-[1-9][0-9]*-attempt-[1-9][0-9]*\.json",
            name,
        )
        is not None
    }
    required_names = set(expected) | custody | supplemental
    if set(actual) != required_names:
        raise ValueError(
            "downloaded suite asset set is not exact: "
            f"missing={sorted(required_names - set(actual))}, "
            f"unexpected={sorted(set(actual) - required_names)}"
        )
    for name, checksum in expected.items():
        if sha256(actual[name]) != checksum:
            raise ValueError(f"downloaded suite asset checksum differs: {name}")
    record = json.loads(record_path.read_text(encoding="utf-8"))
    required_fields = {
        "assets",
        "attestations",
        "daemon_archives",
        "flasher",
        "image",
        "inventory",
        "linkage",
        "railway",
        "release",
        "sboms",
        "schema",
        "trust_root",
    }
    if (
        not isinstance(record, dict)
        or set(record) != required_fields
        or record.get("schema") != 2
        or record.get("release")
        != {"source_commit": commit, "version": version}
    ):
        raise ValueError("suite release record has an unsupported identity or shape")
    expected_record = suite_record_value(root, inventory, commit)
    if record != expected_record:
        raise ValueError("suite release record differs from the exact inventoried assets")
    if record.get("inventory") != file_identity(inventory):
        raise ValueError("suite release record does not bind the checksum inventory")
    expected_assets = [
        {
            "name": name,
            "sha256": checksum,
            "size": actual[name].stat().st_size,
        }
        for name, checksum in sorted(expected.items())
    ]
    if record.get("assets") != expected_assets:
        raise ValueError("suite release record asset evidence differs from the inventory")
    image = record.get("image")
    if not isinstance(image, dict) or image.get("manifest_digest") != arguments.image_digest:
        raise ValueError("suite release record differs from the required OCI digest")
    metadata_name = image.get("metadata")
    if not isinstance(metadata_name, str) or metadata_name not in expected:
        raise ValueError("suite release record has no inventoried image metadata")
    metadata = json.loads(actual[metadata_name].read_text(encoding="utf-8"))
    if (
        metadata.get("source_commit") != commit
        or metadata.get("version") != version
        or metadata.get("manifest_digest") != arguments.image_digest
    ):
        raise ValueError("signed image metadata differs from the required release")
    print(
        f"verified unified suite {version} at {commit} "
        f"with OCI manifest {arguments.image_digest}"
    )


def write_deployment_evidence(arguments: argparse.Namespace) -> None:
    commit = require_commit(arguments.source_commit)
    if SHA256_PATTERN.fullmatch(arguments.image_digest) is None:
        raise ValueError("deployment image digest must be one OCI SHA-256 digest")
    if (
        IDENTITY_HASH_PATTERN.fullmatch(arguments.identity_before) is None
        or IDENTITY_HASH_PATTERN.fullmatch(arguments.identity_after) is None
        or arguments.identity_before != arguments.identity_after
    ):
        raise ValueError("deployment identities must be equal lowercase 16-byte hashes")
    for label, value in (
        ("template revision", arguments.template_revision),
        ("rollback revision", arguments.rollback_revision),
        ("public endpoint", arguments.public_endpoint),
        ("observed timestamp", arguments.observed_at),
    ):
        if (
            not value
            or value != value.strip()
            or any(ord(character) < 0x20 for character in value)
        ):
            raise ValueError(f"{label} must be one nonempty printable value")
    if arguments.workflow_run_id <= 0 or arguments.workflow_run_attempt <= 0:
        raise ValueError("deployment qualification run identity must be positive")
    value = {
        "checks": {
            "backbone_publicly_reachable": True,
            "digest_pinned": True,
            "identity_stable": True,
            "persistence_restored": True,
            "rollback_completed": True,
            "single_replica": True,
        },
        "deployment": {
            "identity": arguments.identity_before,
            "public_endpoint": arguments.public_endpoint,
            "rollback_revision": arguments.rollback_revision,
            "template_revision": arguments.template_revision,
        },
        "image_digest": arguments.image_digest,
        "observed_at": arguments.observed_at,
        "repository": arguments.repository,
        "schema": 1,
        "source_commit": commit,
        "version": suite_version(),
        "workflow": {
            "path": ".github/workflows/suite-deployment-qualification.yml",
            "run_attempt": arguments.workflow_run_attempt,
            "run_id": arguments.workflow_run_id,
        },
    }
    arguments.output.write_bytes(canonical_json(value))
    print(f"wrote protected deployment qualification {arguments.output}")


def verify_deployment_evidence(arguments: argparse.Namespace) -> None:
    evidence_path = regular_file(arguments.evidence, "deployment qualification evidence")
    if re.fullmatch(r"[0-9a-f]{64}", arguments.evidence_sha256) is None:
        raise ValueError("deployment evidence SHA-256 is malformed")
    if sha256(evidence_path) != arguments.evidence_sha256:
        raise ValueError("deployment qualification evidence differs from its recorded SHA-256")
    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    expected_checks = {
        "backbone_publicly_reachable": True,
        "digest_pinned": True,
        "identity_stable": True,
        "persistence_restored": True,
        "rollback_completed": True,
        "single_replica": True,
    }
    workflow = evidence.get("workflow") if isinstance(evidence, dict) else None
    if (
        not isinstance(evidence, dict)
        or not isinstance(workflow, dict)
        or evidence.get("schema") != 1
        or evidence.get("version") != suite_version()
        or evidence.get("source_commit") != require_commit(arguments.source_commit)
        or evidence.get("image_digest") != arguments.image_digest
        or evidence.get("repository") != arguments.repository
        or evidence.get("checks") != expected_checks
        or workflow.get("run_id") != arguments.workflow_run_id
    ):
        raise ValueError("deployment qualification evidence differs from the required release")
    print(f"verified protected deployment qualification {evidence_path.name}")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    commands = root.add_subparsers(dest="command", required=True)

    archive = commands.add_parser("archive")
    archive.add_argument("--binary", type=Path, required=True)
    archive.add_argument("--target", choices=sorted(TARGETS), required=True)
    archive.add_argument("--source-commit", required=True)
    archive.add_argument("--source-date-epoch", type=int, required=True)
    archive.add_argument("--rustc", required=True)
    archive.add_argument("--source-archive", type=Path, required=True)
    archive.add_argument("--source-checksum", type=Path, required=True)
    archive.add_argument("--output", type=Path, required=True)
    archive.set_defaults(run=build_archive)

    compare = commands.add_parser("compare")
    compare.add_argument("--primary", type=Path, required=True)
    compare.add_argument("--reproduction", type=Path, required=True)
    compare.set_defaults(run=compare_files)

    oci_digest = commands.add_parser("oci-digest")
    oci_digest.add_argument("--layout", type=Path, required=True)
    oci_digest.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], required=True)
    oci_digest.set_defaults(run=print_oci_digest)

    oci_compare = commands.add_parser("oci-compare")
    oci_compare.add_argument("--primary", type=Path, required=True)
    oci_compare.add_argument("--reproduction", type=Path, required=True)
    oci_compare.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], required=True)
    oci_compare.set_defaults(run=compare_oci)

    image = commands.add_parser("image-metadata")
    image.add_argument("--source-commit", required=True)
    image.add_argument("--manifest-digest", required=True)
    image.add_argument("--platform-digest", action="append", default=[], required=True)
    image.add_argument("--output", type=Path, required=True)
    image.set_defaults(run=write_image_metadata)

    railway = commands.add_parser("railway-contract")
    railway.add_argument("--source-commit", required=True)
    railway.add_argument("--image-digest", required=True)
    railway.add_argument("--output", type=Path, required=True)
    railway.set_defaults(run=write_railway_contract)

    candidate = commands.add_parser("candidate-index")
    candidate.add_argument("--assets", type=Path, required=True)
    candidate.add_argument("--source-commit", required=True)
    candidate.add_argument("--repository", required=True)
    candidate.add_argument("--workflow-run-id", type=int, required=True)
    candidate.add_argument("--workflow-run-attempt", type=int, required=True)
    candidate.add_argument("--output", type=Path, required=True)
    candidate.set_defaults(run=write_candidate_index)

    candidate_verify = commands.add_parser("candidate-verify")
    candidate_verify.add_argument("--assets", type=Path, required=True)
    candidate_verify.add_argument("--index", type=Path, required=True)
    candidate_verify.add_argument("--source-commit", required=True)
    candidate_verify.add_argument("--repository", required=True)
    candidate_verify.add_argument("--workflow-run-id", type=int, required=True)
    candidate_verify.set_defaults(run=verify_candidate_index)

    image_candidate = commands.add_parser("image-candidate-index")
    image_candidate.add_argument("--assets", type=Path, required=True)
    image_candidate.add_argument("--source-commit", required=True)
    image_candidate.add_argument("--source-archive-checksum", type=Path, required=True)
    image_candidate.add_argument("--repository", required=True)
    image_candidate.add_argument("--workflow-run-id", type=int, required=True)
    image_candidate.add_argument("--workflow-run-attempt", type=int, required=True)
    image_candidate.add_argument("--output", type=Path, required=True)
    image_candidate.set_defaults(run=write_image_candidate_index)

    image_candidate_verify = commands.add_parser("image-candidate-verify")
    image_candidate_verify.add_argument("--assets", type=Path, required=True)
    image_candidate_verify.add_argument("--index", type=Path, required=True)
    image_candidate_verify.add_argument("--source-commit", required=True)
    image_candidate_verify.add_argument("--repository", required=True)
    image_candidate_verify.add_argument("--workflow-run-id", type=int, required=True)
    image_candidate_verify.set_defaults(run=verify_image_candidate_index)

    inventory = commands.add_parser("inventory")
    inventory_commands = inventory.add_subparsers(dest="inventory_command", required=True)
    create = inventory_commands.add_parser("create")
    create.add_argument("--assets", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    create.set_defaults(run=create_inventory)
    verify = inventory_commands.add_parser("verify")
    verify.add_argument("--assets", type=Path, required=True)
    verify.add_argument("--inventory", type=Path, required=True)
    verify.set_defaults(run=verify_inventory)

    record = commands.add_parser("suite-record")
    record.add_argument("--assets", type=Path, required=True)
    record.add_argument("--inventory", type=Path, required=True)
    record.add_argument("--source-commit", required=True)
    record.add_argument("--output", type=Path, required=True)
    record.set_defaults(run=write_suite_record)

    suite_verify = commands.add_parser("suite-verify")
    suite_verify.add_argument("--assets", type=Path, required=True)
    suite_verify.add_argument("--source-commit", required=True)
    suite_verify.add_argument("--image-digest", required=True)
    suite_verify.set_defaults(run=verify_suite_release)

    deployment = commands.add_parser("deployment-evidence")
    deployment.add_argument("--source-commit", required=True)
    deployment.add_argument("--image-digest", required=True)
    deployment.add_argument("--repository", required=True)
    deployment.add_argument("--template-revision", required=True)
    deployment.add_argument("--rollback-revision", required=True)
    deployment.add_argument("--public-endpoint", required=True)
    deployment.add_argument("--identity-before", required=True)
    deployment.add_argument("--identity-after", required=True)
    deployment.add_argument("--observed-at", required=True)
    deployment.add_argument("--workflow-run-id", type=int, required=True)
    deployment.add_argument("--workflow-run-attempt", type=int, required=True)
    deployment.add_argument("--output", type=Path, required=True)
    deployment.set_defaults(run=write_deployment_evidence)

    deployment_verify = commands.add_parser("deployment-verify")
    deployment_verify.add_argument("--evidence", type=Path, required=True)
    deployment_verify.add_argument("--evidence-sha256", required=True)
    deployment_verify.add_argument("--source-commit", required=True)
    deployment_verify.add_argument("--image-digest", required=True)
    deployment_verify.add_argument("--repository", required=True)
    deployment_verify.add_argument("--workflow-run-id", type=int, required=True)
    deployment_verify.set_defaults(run=verify_deployment_evidence)
    return root


def main() -> int:
    arguments = parser().parse_args()
    try:
        arguments.run(arguments)
    except (OSError, ValueError, json.JSONDecodeError, zipfile.BadZipFile) as error:
        print(f"prnsd distribution failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
