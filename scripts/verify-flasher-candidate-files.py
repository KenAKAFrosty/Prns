#!/usr/bin/env python3
"""Verify the signed candidate checksum inventory identically on every host OS."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import sys


CHECKSUM_LINE = re.compile(r"^([0-9a-f]{64})  ([^\r\n]+)$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_relative_path(raw: str) -> PurePosixPath:
    if (
        not raw
        or "\\" in raw
        or "//" in raw
        or any(ord(character) < 0x20 for character in raw)
    ):
        raise ValueError(f"unsafe checksum path: {raw!r}")
    components = raw.split("/")
    if any(component in {"", ".", ".."} for component in components):
        raise ValueError(f"unsafe checksum path: {raw!r}")
    relative = PurePosixPath(raw)
    if relative.is_absolute() or relative.as_posix() != raw:
        raise ValueError(f"unsafe checksum path: {raw!r}")
    return relative


def historical_signature_paths(
    root: Path, listed: dict[str, str], current_version: str
) -> set[str]:
    metadata_relative = "metadata/release-history.json"
    metadata_path = root / metadata_relative
    if not metadata_path.exists():
        return set()
    if metadata_relative not in listed:
        raise ValueError("release-history metadata is not covered by SHA256SUMS.txt")
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    files = metadata.get("files") if isinstance(metadata, dict) else None
    if not isinstance(files, list):
        raise ValueError("release-history metadata has no file inventory")
    allowed: set[str] = set()
    for index, item in enumerate(files):
        if not isinstance(item, dict):
            raise ValueError(f"release-history file identity {index} is malformed")
        raw_relative = item.get("path")
        size = item.get("size")
        expected = item.get("sha256")
        if not isinstance(raw_relative, str):
            raise ValueError(f"release-history file identity {index} has no path")
        relative = safe_relative_path(raw_relative)
        if len(relative.parts) < 2 or relative.parts[0] == current_version:
            raise ValueError("release-history file path is not a prior immutable version")
        if not raw_relative.endswith(".minisig"):
            continue
        candidate_relative = f"website/releases/{raw_relative}"
        path = root.joinpath(*PurePosixPath(candidate_relative).parts)
        if (
            not isinstance(size, int)
            or isinstance(size, bool)
            or size < 0
            or not isinstance(expected, str)
            or CHECKSUM_LINE.fullmatch(f"{expected}  x") is None
            or not path.is_file()
            or path.stat().st_size != size
            or sha256(path) != expected
        ):
            raise ValueError(
                f"historical signature differs from release-history metadata: {raw_relative}"
            )
        allowed.add(candidate_relative)
    return allowed


def verify(candidate: Path) -> None:
    if candidate.is_symlink() or not candidate.is_dir():
        raise ValueError(f"candidate must be a real directory: {candidate}")
    root = candidate.resolve()
    all_paths = list(root.rglob("*"))
    symlinks = [path.relative_to(root).as_posix() for path in all_paths if path.is_symlink()]
    if symlinks:
        raise ValueError(f"candidate contains symlinks: {symlinks}")
    actual_files = {
        path.relative_to(root).as_posix() for path in all_paths if path.is_file()
    }

    checksum_path = root / "SHA256SUMS.txt"
    lines = checksum_path.read_text(encoding="utf-8").splitlines()
    if not lines:
        raise ValueError("SHA256SUMS.txt is empty")
    listed: dict[str, str] = {}
    for index, line in enumerate(lines, start=1):
        matched = CHECKSUM_LINE.fullmatch(line)
        if matched is None:
            raise ValueError(f"malformed SHA256SUMS.txt line {index}")
        expected, raw_relative = matched.groups()
        relative = safe_relative_path(raw_relative).as_posix()
        if relative in listed:
            raise ValueError(f"duplicate checksum path: {relative}")
        if relative == "SHA256SUMS.txt":
            raise ValueError("SHA256SUMS.txt cannot checksum itself")
        listed[relative] = expected

    for relative, expected in listed.items():
        path = root.joinpath(*PurePosixPath(relative).parts)
        if not path.is_file():
            raise ValueError(f"checksummed file is missing: {relative}")
        actual = sha256(path)
        if actual != expected:
            raise ValueError(f"SHA-256 mismatch: {relative}")

    version = (root / "VERSION").read_text(encoding="utf-8").strip()
    channel_documents = sorted((root / "channels").glob("*.json"))
    if len(channel_documents) != 1:
        raise ValueError("candidate must contain exactly one channel descriptor")
    channel = channel_documents[0].stem
    if (
        not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+-]*", version)
        or version.lower() == "next"
        or channel not in {"stable", "preview"}
    ):
        raise ValueError("candidate version/channel is not an immutable release identity")
    allowed_unlisted = {
        "SHA256SUMS.txt",
        "SHA256SUMS.txt.minisig",
        "flash-manifest.json.minisig",
        f"channels/{channel}.json.minisig",
        f"website/releases/{version}/flash-manifest.json.minisig",
        f"website/releases/channels/{channel}.json.minisig",
    }
    allowed_unlisted.update(historical_signature_paths(root, listed, version))
    unlisted = actual_files - set(listed)
    if unlisted != allowed_unlisted:
        missing = sorted(allowed_unlisted - unlisted)
        unexpected = sorted(unlisted - allowed_unlisted)
        raise ValueError(
            f"candidate checksum inventory differs; missing={missing}, unexpected={unexpected}"
        )
    if (root / "flash-manifest.json.minisig").read_bytes() != (
        root / "website" / "releases" / version / "flash-manifest.json.minisig"
    ).read_bytes():
        raise ValueError("hosted manifest signature differs from candidate signature")
    if (root / "channels" / f"{channel}.json.minisig").read_bytes() != (
        root / "website" / "releases" / "channels" / f"{channel}.json.minisig"
    ).read_bytes():
        raise ValueError("hosted channel signature differs from candidate signature")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", type=Path)
    arguments = parser.parse_args()
    try:
        verify(arguments.candidate)
    except (OSError, UnicodeError, ValueError) as error:
        print(f"candidate file verification failed: {error}", file=sys.stderr)
        return 1
    print("candidate checksum inventory is complete and verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
