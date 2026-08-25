"""Cumulative immutable website-release history for flasher candidates."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil

from flasher_manifest import FLASH_MANIFEST_SCHEMA


FLASHER_RELEASE_RECORD_NAME = re.compile(r"flasher-release-record-v.+\.json")
SIGNED_CANDIDATE_NAME = re.compile(r"prns-flasher-candidate-v.+-signed\.tar\.gz")
RETAINED_FLASH_MANIFEST_SCHEMAS = frozenset({2, FLASH_MANIFEST_SCHEMA})


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_version(value: object) -> str:
    if not isinstance(value, str) or not value or value.lower() == "next":
        raise ValueError("release-history version is missing or unresolved")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or len(path.parts) != 1
        or path.as_posix() != value
        or any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+" for character in value)
    ):
        raise ValueError(f"release-history version is not path-safe: {value!r}")
    return value


def require_sha256(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(
        character not in "0123456789abcdef" for character in value
    ):
        raise ValueError(f"{label} must be a lowercase SHA-256")
    return value


def require_commit(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 40 or any(
        character not in "0123456789abcdef" for character in value
    ):
        raise ValueError(f"{label} must be a lowercase full Git commit")
    return value


def tree_files(root: Path) -> list[dict[str, str | int]]:
    if not root.is_dir():
        return []
    files = []
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"release history cannot contain a symlink: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"release history contains an unsupported entry: {path}")
        relative = path.relative_to(root).as_posix()
        pure = PurePosixPath(relative)
        if pure.is_absolute() or any(part in {"", ".", ".."} for part in pure.parts):
            raise ValueError(f"release history contains an unsafe path: {relative}")
        files.append(
            {
                "path": relative,
                "size": path.stat().st_size,
                "sha256": sha256(path),
            }
        )
    return sorted(files, key=lambda item: str(item["path"]))


def tree_identity(files: list[dict[str, str | int]]) -> dict[str, str | int]:
    digest = hashlib.sha256()
    total = 0
    for item in files:
        size = item["size"]
        checksum = item["sha256"]
        relative = item["path"]
        if not isinstance(size, int) or not isinstance(checksum, str) or not isinstance(relative, str):
            raise ValueError("release-history tree entry is malformed")
        total += size
        digest.update(f"{checksum}  {size}  {relative}\n".encode())
    return {
        "file_count": len(files),
        "total_bytes": total,
        "tree_sha256": digest.hexdigest(),
    }


def write_metadata(output: Path, mode: str, head: dict | None) -> dict:
    releases = output / "releases"
    files = tree_files(releases)
    metadata = {
        "schema": 1,
        "mode": mode,
        "head": head,
        "tree": tree_identity(files),
        "files": files,
    }
    (output / "history.json").write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return metadata


def require_empty_output(output: Path) -> None:
    if output.exists():
        if not output.is_dir() or any(output.iterdir()):
            raise ValueError("release-history output must be a new or empty directory")
    output.mkdir(parents=True, exist_ok=True)


def bootstrap_blocking_custody_tags(releases: object) -> list[str]:
    """Return releases proving that stable website history already exists."""

    if not isinstance(releases, list):
        raise ValueError("GitHub releases response must be a JSON array")
    blocking = set()
    for release in releases:
        if not isinstance(release, dict):
            raise ValueError("GitHub releases response contains a malformed release")
        tag = release.get("tag_name")
        draft = release.get("draft")
        prerelease = release.get("prerelease")
        assets = release.get("assets")
        if (
            not isinstance(tag, str)
            or not tag
            or not isinstance(draft, bool)
            or not isinstance(prerelease, bool)
            or not isinstance(assets, list)
        ):
            raise ValueError(f"GitHub release metadata is malformed for {tag!r}")
        names = []
        for asset in assets:
            name = asset.get("name") if isinstance(asset, dict) else None
            if not isinstance(name, str) or not name:
                raise ValueError(f"GitHub release asset metadata is malformed for {tag}")
            names.append(name)
        has_flasher_release_record = any(
            FLASHER_RELEASE_RECORD_NAME.fullmatch(name) for name in names
        )
        has_signed_candidate = any(
            SIGNED_CANDIDATE_NAME.fullmatch(name) for name in names
        )
        if has_flasher_release_record or (
            not draft and not prerelease and has_signed_candidate
        ):
            blocking.add(tag)
    return sorted(blocking)


def prepare_bootstrap(output: Path) -> dict:
    require_empty_output(output)
    return write_metadata(output, "bootstrap", None)


def load_object(path: Path, label: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def require_retained_schema(manifest: dict) -> None:
    if manifest.get("schema") not in RETAINED_FLASH_MANIFEST_SCHEMAS:
        supported = ", ".join(
            str(schema) for schema in sorted(RETAINED_FLASH_MANIFEST_SCHEMAS)
        )
        raise ValueError(f"retained flash manifest must use schema {supported}")


def validate_metadata_identity(
    metadata: dict,
    *,
    actual_files: list[dict[str, str | int]],
    release_directories: set[str],
    releases: Path,
) -> None:
    if set(metadata) != {"schema", "mode", "head", "tree", "files"} or metadata.get("schema") != 1:
        raise ValueError("release-history metadata has an unsupported shape")
    mode = metadata.get("mode")
    head = metadata.get("head")
    if mode == "bootstrap":
        if head is not None:
            raise ValueError("bootstrap release history cannot name a prior release")
    elif mode == "retained":
        if not isinstance(head, dict) or set(head) != {
            "version",
            "source_commit",
            "manifest_sha256",
            "release_record_sha256",
            "signed_bundle_sha256",
        }:
            raise ValueError("retained release-history head is malformed")
        canonical_version(head.get("version"))
        require_commit(head.get("source_commit"), "release-history source commit")
        for field in ("manifest_sha256", "release_record_sha256", "signed_bundle_sha256"):
            require_sha256(head.get(field), f"release-history {field}")
    else:
        raise ValueError("release-history mode must be bootstrap or retained")
    files = metadata.get("files")
    if not isinstance(files, list) or files != actual_files:
        raise ValueError("release-history file identities differ from retained bytes")
    if metadata.get("tree") != tree_identity(files):
        raise ValueError("release-history tree identity differs from retained bytes")
    versions = set()
    for item in files:
        relative = item.get("path") if isinstance(item, dict) else None
        if not isinstance(relative, str):
            raise ValueError("release-history file entry is malformed")
        version = canonical_version(PurePosixPath(relative).parts[0])
        versions.add(version)
    if release_directories != versions:
        raise ValueError("release-history directories differ from retained file identities")
    if mode == "bootstrap" and files:
        raise ValueError("bootstrap release history must be empty")
    if mode == "retained" and head["version"] not in versions:
        raise ValueError("release-history head is absent from retained versions")
    for version in versions:
        directory = releases / version
        for required in ("flash-manifest.json", "flash-manifest.json.minisig"):
            if not (directory / required).is_file():
                raise ValueError(f"retained release {version} lacks {required}")
        manifest = load_object(directory / "flash-manifest.json", "retained manifest")
        release = manifest.get("release")
        require_retained_schema(manifest)
        if not isinstance(release, dict) or release.get("version") != version:
            raise ValueError(f"retained release directory {version} has the wrong manifest")


def validate_metadata(metadata: dict, releases: Path) -> None:
    release_directories = set()
    if releases.exists():
        if not releases.is_dir() or releases.is_symlink():
            raise ValueError("release-history releases entry is not a directory")
        for child in releases.iterdir():
            canonical_version(child.name)
            if not child.is_dir() or child.is_symlink():
                raise ValueError(f"release-history version entry is invalid: {child}")
            release_directories.add(child.name)
    validate_metadata_identity(
        metadata,
        actual_files=tree_files(releases),
        release_directories=release_directories,
        releases=releases,
    )


def load_history(root: Path) -> dict:
    metadata = load_object(root / "history.json", "release-history metadata")
    validate_metadata(metadata, root / "releases")
    return metadata


def candidate_version(
    candidate: Path, *, allow_retained_schema: bool = False
) -> tuple[str, dict]:
    manifest = load_object(candidate / "flash-manifest.json", "candidate manifest")
    release = manifest.get("release")
    schema = manifest.get("schema")
    supported_schemas = (
        RETAINED_FLASH_MANIFEST_SCHEMAS
        if allow_retained_schema
        else frozenset({FLASH_MANIFEST_SCHEMA})
    )
    if schema not in supported_schemas or not isinstance(release, dict):
        raise ValueError(f"candidate manifest is not schema {FLASH_MANIFEST_SCHEMA}")
    return canonical_version(release.get("version")), manifest


def historical_release_root(candidate: Path, current_version: str) -> Path:
    source = candidate / "website" / "releases"
    if not source.exists():
        return source
    if not source.is_dir() or source.is_symlink():
        raise ValueError("candidate website release directory is unavailable")
    for child in source.iterdir():
        if child.name in {"channels", "minisign.pub", current_version}:
            continue
        canonical_version(child.name)
        if not child.is_dir() or child.is_symlink():
            raise ValueError(f"candidate historical release entry is invalid: {child}")
    return source


def validate_candidate_history(
    candidate: Path, *, allow_retained_schema: bool = False
) -> dict:
    current_version, _ = candidate_version(
        candidate, allow_retained_schema=allow_retained_schema
    )
    metadata = load_object(
        candidate / "metadata" / "release-history.json", "candidate release-history metadata"
    )
    source = historical_release_root(candidate, current_version)
    historical_versions = (
        {
            child.name
            for child in source.iterdir()
            if child.is_dir() and child.name not in {"channels", current_version}
        }
        if source.is_dir()
        else set()
    )
    staging = candidate / "metadata" / ".release-history-validation"
    if staging.exists():
        raise ValueError("candidate contains a reserved release-history validation path")
    actual_files = []
    for version in sorted(historical_versions):
        directory = source / version
        for path in directory.rglob("*"):
            if path.is_symlink():
                raise ValueError(f"candidate retained release contains a symlink: {path}")
            if path.is_dir():
                continue
            if not path.is_file():
                raise ValueError(f"candidate retained release has an unsupported entry: {path}")
            relative = path.relative_to(source).as_posix()
            actual_files.append(
                {"path": relative, "size": path.stat().st_size, "sha256": sha256(path)}
            )
    validate_metadata_identity(
        metadata,
        actual_files=sorted(actual_files, key=lambda item: str(item["path"])),
        release_directories=historical_versions,
        releases=source,
    )
    return metadata


def stable_descriptor_identity(path: Path) -> dict[str, str] | None:
    """Return a canonical stable identity; only an HTML fallback is absent."""

    document = path.read_text(encoding="utf-8")
    normalized = document.lstrip().lower()
    if normalized.startswith("<!doctype html") or normalized.startswith("<html"):
        return None
    try:
        descriptor = json.loads(document)
    except json.JSONDecodeError as error:
        raise ValueError("stable channel response is neither JSON nor HTML") from error
    expected_fields = {
        "schema",
        "channel",
        "version",
        "manifest_url",
        "manifest_sha256",
    }
    if not isinstance(descriptor, dict) or set(descriptor) != expected_fields:
        raise ValueError("stable channel descriptor has an unsupported shape")
    if descriptor.get("schema") != 1 or descriptor.get("channel") != "stable":
        raise ValueError("stable channel descriptor has the wrong schema or channel")
    version = canonical_version(descriptor.get("version"))
    manifest_sha256 = require_sha256(
        descriptor.get("manifest_sha256"), "stable manifest SHA-256"
    )
    if descriptor.get("manifest_url") != (
        f"https://reticulum.rs/releases/{version}/flash-manifest.json"
    ):
        raise ValueError("stable channel descriptor has a mutable or foreign manifest URL")
    return {"version": version, "manifest_sha256": manifest_sha256}


def prepare_retained(candidate: Path, release_record: Path, output: Path) -> dict:
    require_empty_output(output)
    version, manifest = candidate_version(candidate, allow_retained_schema=True)
    validate_candidate_history(candidate, allow_retained_schema=True)
    record = load_object(release_record, "release record")
    release = record.get("release")
    archive = record.get("candidate", {}).get("archive") if isinstance(record.get("candidate"), dict) else None
    candidate_manifest = record.get("candidate", {}).get("manifest") if isinstance(record.get("candidate"), dict) else None
    manifest_release = manifest.get("release")
    if not isinstance(manifest_release, dict) or manifest_release.get("channel") != "stable":
        raise ValueError("only a signed stable candidate can seed release history")
    if (
        not isinstance(release, dict)
        or release.get("version") != version
        or release.get("channel") != "stable"
    ):
        raise ValueError("release record differs from the retained candidate")
    if not isinstance(archive, dict) or not isinstance(candidate_manifest, dict):
        raise ValueError("release record lacks retained candidate identities")
    source = historical_release_root(candidate, version)
    releases = output / "releases"
    releases.mkdir()
    for child in sorted(source.iterdir(), key=lambda path: path.name):
        if not child.is_dir() or child.name == "channels":
            continue
        canonical_version(child.name)
        shutil.copytree(
            child,
            releases / child.name,
            symlinks=True,
            copy_function=shutil.copy2,
        )
    head = {
        "version": version,
        "source_commit": require_commit(release.get("source_commit"), "release source commit"),
        "manifest_sha256": require_sha256(
            candidate_manifest.get("sha256"), "release manifest SHA-256"
        ),
        "release_record_sha256": sha256(release_record),
        "signed_bundle_sha256": require_sha256(
            archive.get("sha256"), "signed candidate bundle SHA-256"
        ),
    }
    manifest_hash = sha256(candidate / "flash-manifest.json")
    if head["manifest_sha256"] != manifest_hash:
        raise ValueError("release record manifest hash differs from the retained candidate")
    return write_metadata(output, "retained", head)


def apply_history(history: Path, candidate: Path) -> dict:
    metadata = load_history(history)
    website_releases = candidate / "website" / "releases"
    if website_releases.exists():
        raise ValueError("candidate website already contains release history")
    releases = history / "releases"
    if releases.is_dir():
        shutil.copytree(
            releases,
            website_releases,
            symlinks=True,
            copy_function=shutil.copy2,
        )
    metadata_path = candidate / "metadata" / "release-history.json"
    metadata_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(history / "history.json", metadata_path)
    return metadata


def allowed_historical_signatures(candidate: Path) -> set[str]:
    metadata = validate_candidate_history(candidate)
    return {
        f"website/releases/{item['path']}"
        for item in metadata["files"]
        if str(item["path"]).endswith(".minisig")
    }
