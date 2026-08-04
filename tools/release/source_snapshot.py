"""Deterministic, commit-bound source snapshots for the hosted website."""

from __future__ import annotations

import hashlib
import io
import json
import os
import stat
import subprocess
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

MAX_ARCHIVE_FILES = 100_000
MAX_UNCOMPRESSED_BYTES = 2 * 1024 * 1024 * 1024
ARCHIVE_TIMEZONE = "UTC"
REQUIRED_SOURCE_FILES = (
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "VERSION",
    "docs/website/Cargo.toml",
    "docs/website/Dioxus.toml",
    "docs/website/build.rs",
    "docs/website/package-lock.json",
    "docs/website/package.json",
    "docs/website/src/main.rs",
    "assets/nnpages/coming_from_rns.mu",
    "assets/nnpages/credits.mu",
    "assets/nnpages/license.mu",
    "assets/nnpages/masthead.mu",
    "assets/nnpages/nav.mu",
    "assets/nnpages/quote.mu",
    "assets/nnpages/source_available.mu",
    "assets/nnpages/why_prns.mu",
    "personal-hopspot/core/src/node_pages.rs",
    "personal-hopspot/core/src/node_pages/browser_welcome.mu",
    "personal-hopspot/core/src/node_pages/hopspot_welcome.mu",
    "personal-hopspot/core/src/node_pages/source_missing.mu",
)


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def archive_prefix(version: str) -> str:
    if (
        not version
        or version.lower() == "next"
        or any(
            character
            not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-_+"
            for character in version
        )
    ):
        raise ValueError("source snapshot version must be a publishable path-safe version")
    return f"Prns-{version}/"


def require_commit(commit: str) -> None:
    if len(commit) != 40 or any(character not in "0123456789abcdef" for character in commit):
        raise ValueError("source snapshot commit must be a lowercase full Git commit")


def resolve_commit(repository: Path, commit: str) -> str:
    require_commit(commit)
    if not repository.is_dir():
        raise ValueError(f"source repository does not exist: {repository}")
    result = subprocess.run(
        ("git", "-C", str(repository), "rev-parse", "--verify", f"{commit}^{{commit}}"),
        text=True,
        capture_output=True,
        check=False,
    )
    resolved = result.stdout.strip()
    if result.returncode != 0 or resolved != commit:
        raise ValueError("source snapshot commit is unavailable from the source repository")
    return resolved


def compact_archive(value: bytes) -> bytes:
    output = io.BytesIO()
    with (
        zipfile.ZipFile(io.BytesIO(value), mode="r") as source,
        zipfile.ZipFile(
            output,
            mode="w",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
        ) as destination,
    ):
        for member in source.infolist():
            if member.is_dir():
                continue
            member.extra = b""
            member.comment = b""
            destination.writestr(
                member,
                source.read(member),
                compress_type=zipfile.ZIP_DEFLATED,
                compresslevel=9,
            )
    return output.getvalue()


def git_archive(repository: Path, *, commit: str, version: str) -> bytes:
    resolved = resolve_commit(repository, commit)
    environment = os.environ.copy()
    environment["TZ"] = ARCHIVE_TIMEZONE
    result = subprocess.run(
        (
            "git",
            "-C",
            str(repository),
            "archive",
            "--format=zip",
            f"--prefix={archive_prefix(version)}",
            resolved,
        ),
        capture_output=True,
        check=False,
        env=environment,
    )
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise ValueError(f"git archive failed for source snapshot: {detail}")
    if not result.stdout:
        raise ValueError("git archive produced an empty source snapshot")
    return compact_archive(result.stdout)


def validate_archive_members(value: bytes, *, version: str) -> None:
    prefix = archive_prefix(version)
    with zipfile.ZipFile(io.BytesIO(value), mode="r") as archive:
        members = archive.infolist()
        if len(members) > MAX_ARCHIVE_FILES:
            raise ValueError("source snapshot contains too many files")
        if sum(member.file_size for member in members) > MAX_UNCOMPRESSED_BYTES:
            raise ValueError("source snapshot exceeds the uncompressed size limit")
        names: set[str] = set()
        files: set[str] = set()
        for member in members:
            relative = member.filename
            pure = PurePosixPath(relative)
            mode = member.external_attr >> 16
            if (
                "\\" in relative
                or pure.is_absolute()
                or not pure.parts
                or any(part in {"", ".", ".."} for part in pure.parts)
                or not relative.startswith(prefix)
                or stat.S_ISLNK(mode)
            ):
                raise ValueError(f"source snapshot contains an unsafe member: {relative!r}")
            if relative in names:
                raise ValueError(f"source snapshot contains a duplicate member: {relative}")
            names.add(relative)
            if not member.is_dir():
                files.add(relative.removeprefix(prefix))

        missing = sorted(set(REQUIRED_SOURCE_FILES) - files)
        if missing:
            raise ValueError(
                "source snapshot omits required website or NomadNet sources: "
                f"{missing}"
            )
        try:
            archived_version = archive.read(f"{prefix}VERSION").decode("utf-8").strip()
        except UnicodeDecodeError as error:
            raise ValueError("source snapshot VERSION is not UTF-8") from error
        if archived_version != version:
            raise ValueError("source snapshot VERSION differs from its archive identity")


def atomic_write(path: Path, value: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink():
        raise ValueError(f"refusing to replace source snapshot symlink: {path}")
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            prefix=f".{path.name}.",
            suffix=".tmp",
            dir=path.parent,
            delete=False,
        ) as stream:
            temporary = Path(stream.name)
            stream.write(value)
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def output_path(path: Path) -> Path:
    absolute = path if path.is_absolute() else Path.cwd() / path
    return absolute.parent.resolve() / absolute.name


def checksum_document(archive: Path, value: bytes) -> bytes:
    return f"{sha256_bytes(value)}  {archive.name}\n".encode()


def metadata_document(*, commit: str, version: str, archive: Path, value: bytes) -> bytes:
    document = {
        "schema": 1,
        "artifact": archive.name,
        "version": version,
        "commit": commit,
        "size": len(value),
        "sha256": sha256_bytes(value),
        "checksum": f"{archive.name}.sha256",
        "nomadnet_routes": {
            "archive": "/file/source.zip",
            "checksum": "/file/source.zip.sha256",
        },
    }
    return (
        json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
        + "\n"
    ).encode()


def package_source_snapshot(
    *,
    repository: Path,
    commit: str,
    version: str,
    output: Path,
    checksum: Path | None = None,
    metadata: Path | None = None,
) -> tuple[Path, Path]:
    output = output_path(output)
    checksum = (
        output_path(checksum)
        if checksum is not None
        else output.with_name(f"{output.name}.sha256")
    )
    if output == checksum:
        raise ValueError("source snapshot and checksum outputs must be different files")
    value = git_archive(repository.resolve(), commit=commit, version=version)
    validate_archive_members(value, version=version)
    atomic_write(output, value)
    atomic_write(checksum, checksum_document(output, value))
    if metadata is not None:
        atomic_write(
            output_path(metadata),
            metadata_document(
                commit=commit,
                version=version,
                archive=output,
                value=value,
            ),
        )
    return output, checksum


def verify_source_snapshot(
    *,
    repository: Path,
    commit: str,
    version: str,
    archive: Path,
    checksum: Path,
    metadata: Path | None = None,
) -> None:
    value = archive.read_bytes()
    if not value:
        raise ValueError("hosted source snapshot is empty")
    expected_checksum = checksum_document(archive, value)
    if checksum.read_bytes() != expected_checksum:
        raise ValueError("hosted source snapshot checksum is malformed or stale")
    validate_archive_members(value, version=version)
    expected = git_archive(repository.resolve(), commit=commit, version=version)
    if value != expected:
        raise ValueError("hosted source snapshot differs from the exact stamped Git commit")
    if metadata is not None and metadata.read_bytes() != metadata_document(
        commit=commit,
        version=version,
        archive=archive,
        value=value,
    ):
        raise ValueError("source snapshot canonical metadata is malformed or stale")
