#!/usr/bin/env python3
"""Verify an unsigned candidate's identity and complete checksum envelope before signing."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import sys


CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
SHIPPING_BOARDS = {"heltec-v4", "t-beam-supreme", "xiao-esp32-c6", "t-echo"}


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def is_sha256(value: str) -> bool:
    return len(value) == 64 and all(character in "0123456789abcdef" for character in value)


def safe_path(root: Path, relative: str) -> Path:
    pure = PurePosixPath(relative)
    if (
        "\\" in relative
        or pure.is_absolute()
        or not pure.parts
        or any(part in {"", ".", ".."} for part in pure.parts)
    ):
        raise ValueError(f"unsafe candidate path {relative!r}")
    return root.joinpath(*pure.parts)


def payload_files(root: Path) -> set[str]:
    files: set[str] = set()
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"candidate cannot contain symlink {path}")
        if not path.is_file():
            continue
        relative = path.relative_to(root).as_posix()
        if relative == "SHA256SUMS.txt":
            continue
        files.add(relative)
    return files


def verify_sums(root: Path) -> None:
    sums = root / "SHA256SUMS.txt"
    if not sums.is_file():
        raise ValueError("candidate is missing SHA256SUMS.txt")
    listed: dict[str, str] = {}
    for index, line in enumerate(sums.read_text(encoding="utf-8").splitlines(), start=1):
        try:
            checksum, relative = line.split("  ", maxsplit=1)
        except ValueError as error:
            raise ValueError(f"invalid SHA256SUMS line {index}") from error
        if not is_sha256(checksum):
            raise ValueError(f"invalid lowercase SHA-256 on line {index}")
        path = safe_path(root, relative)
        if relative in listed:
            raise ValueError(f"duplicate SHA256SUMS path {relative!r}")
        if not path.is_file() or digest(path) != checksum:
            raise ValueError(f"SHA-256 mismatch for {relative}")
        listed[relative] = checksum
    actual = payload_files(root)
    expected = set(listed)
    if actual != expected:
        raise ValueError(
            "SHA256SUMS coverage differs; "
            f"unlisted={sorted(actual - expected)}, missing-files={sorted(expected - actual)}"
        )


def public_key_id(document: str) -> str:
    lines = document.splitlines()
    prefix = "untrusted comment: minisign public key "
    if not lines or not lines[0].startswith(prefix):
        raise ValueError("pinned Minisign public key has no canonical key ID")
    key_id = lines[0].removeprefix(prefix).strip()
    if len(key_id) != 16 or not all(character in "0123456789abcdefABCDEF" for character in key_id):
        raise ValueError("pinned Minisign public key ID must be 16 hexadecimal digits")
    return key_id.upper()


def verify(arguments: argparse.Namespace) -> dict:
    root = arguments.candidate.resolve()
    if not root.is_dir():
        raise ValueError(f"candidate directory does not exist: {root}")
    signatures = sorted(path.relative_to(root).as_posix() for path in root.rglob("*.minisig"))
    if signatures:
        raise ValueError(f"unsigned candidate already contains signatures: {signatures}")
    for forbidden in ("acceptance.json", "release-record.json"):
        if (root / forbidden).exists():
            raise ValueError(f"unsigned candidate must not contain {forbidden}")

    pinned_key = arguments.pinned_key.read_text(encoding="utf-8")
    if "PRNS_RELEASE_KEY_NOT_CONFIGURED" in pinned_key:
        raise ValueError("repository release key still contains the fail-closed custody marker")
    candidate_key = (root / "minisign.pub").read_text(encoding="utf-8")
    if candidate_key != pinned_key:
        raise ValueError("candidate Minisign public key differs from the repository pin")
    key_id = public_key_id(pinned_key)

    repository_version = arguments.repository_version.read_text(encoding="utf-8").strip()
    version = (root / "VERSION").read_text(encoding="utf-8").strip()
    if version != repository_version or not version or version.lower() == "next":
        raise ValueError("candidate VERSION differs from the publishable repository VERSION")
    if any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+" for character in version):
        raise ValueError("candidate VERSION is not path-safe")
    if not (
        len(arguments.expected_commit) == 40
        and all(character in "0123456789abcdef" for character in arguments.expected_commit)
    ):
        raise ValueError("expected source commit must be a lowercase full Git commit")

    manifest_path = root / "flash-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    release = manifest.get("release")
    signing = manifest.get("signing")
    if manifest.get("schema") != 2 or not isinstance(release, dict) or not isinstance(signing, dict):
        raise ValueError("candidate manifest identity is malformed")
    channel = release.get("channel")
    if release != {
        "version": version,
        "channel": channel,
        "commit": arguments.expected_commit,
    } or channel not in {"stable", "preview"}:
        raise ValueError("candidate manifest release identity disagrees with signing inputs")
    signing_key_id = signing.get("key_id")
    if not isinstance(signing_key_id, str) or signing_key_id.upper() != key_id:
        raise ValueError("candidate manifest signing key differs from the repository pin")

    targets = manifest.get("targets")
    if not isinstance(targets, list):
        raise ValueError("candidate manifest targets must be an array")
    boards = [target.get("board_slug") for target in targets if isinstance(target, dict)]
    if len(boards) != len(SHIPPING_BOARDS) or set(boards) != SHIPPING_BOARDS:
        raise ValueError("candidate manifest does not contain exactly the four shipping boards")
    immutable_root = root / "website" / "releases" / version
    for target in targets:
        if not isinstance(target, dict):
            raise ValueError("candidate manifest contains a malformed target")
        parts = target.get("parts")
        if not isinstance(parts, list) or not parts:
            raise ValueError(f"candidate target {target.get('board_slug')!r} has no firmware parts")
        for part in parts:
            if not isinstance(part, dict):
                raise ValueError("candidate manifest contains a malformed firmware part")
            relative = part.get("path")
            size = part.get("size")
            checksum = part.get("sha256")
            if (
                not isinstance(relative, str)
                or not isinstance(size, int)
                or isinstance(size, bool)
                or not isinstance(checksum, str)
            ):
                raise ValueError("candidate manifest contains a malformed firmware part")
            artifact = safe_path(root, relative)
            hosted = safe_path(immutable_root, relative)
            if not artifact.is_file() or artifact.stat().st_size != size or digest(artifact) != checksum:
                raise ValueError(f"candidate firmware part does not match manifest: {relative}")
            if not hosted.is_file() or hosted.read_bytes() != artifact.read_bytes():
                raise ValueError(f"hosted firmware part differs from candidate payload: {relative}")

    channel_directory = root / "channels"
    channel_files = sorted(channel_directory.glob("*.json")) if channel_directory.is_dir() else []
    if len(channel_files) != 1 or channel_files[0].stem != channel:
        raise ValueError("candidate must contain exactly its declared channel descriptor")
    descriptor_path = channel_files[0]
    descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
    expected_descriptor = {
        "schema": 1,
        "channel": channel,
        "version": version,
        "manifest_url": f"https://reticulum.rs/releases/{version}/flash-manifest.json",
        "manifest_sha256": digest(manifest_path),
    }
    if descriptor != expected_descriptor:
        raise ValueError("candidate channel descriptor disagrees with its exact manifest")
    hosted_manifest = immutable_root / "flash-manifest.json"
    hosted_channel = root / "website" / "releases" / "channels" / f"{channel}.json"
    if hosted_manifest.read_bytes() != manifest_path.read_bytes():
        raise ValueError("hosted manifest differs from the candidate manifest")
    if hosted_channel.read_bytes() != descriptor_path.read_bytes():
        raise ValueError("hosted channel differs from the candidate channel")

    metadata_path = root / "metadata" / "build.json"
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    if (
        not isinstance(metadata, dict)
        or metadata.get("schema") != 1
        or metadata.get("source_commit") != arguments.expected_commit
    ):
        raise ValueError("candidate build metadata disagrees with its source commit")
    audit_path = root / "audit" / "release-audit-evidence.md"
    if not audit_path.is_file() or not audit_path.read_bytes():
        raise ValueError("candidate lacks release dependency audit evidence")
    for target, extension in CLI_TARGETS.items():
        archive = root / "cli" / f"hopspot-flash-{version}-{target}{extension}"
        if not archive.is_file() or archive.stat().st_size == 0:
            raise ValueError(f"candidate lacks CLI archive for {target}")

    verify_sums(root)
    return {
        "version": version,
        "channel": channel,
        "source_commit": arguments.expected_commit,
        "signing_key_id": key_id,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--expected-commit", required=True)
    parser.add_argument("--repository-version", type=Path, required=True)
    parser.add_argument("--pinned-key", type=Path, required=True)
    parser.add_argument("--identity-output", type=Path)
    arguments = parser.parse_args()
    try:
        identity = verify(arguments)
        if arguments.identity_output:
            arguments.identity_output.parent.mkdir(parents=True, exist_ok=True)
            arguments.identity_output.write_text(
                json.dumps(identity, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
                newline="\n",
            )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"unsigned candidate validation failed: {error}", file=sys.stderr)
        return 1
    print(
        f"verified unsigned candidate {identity['version']} from {identity['source_commit']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
