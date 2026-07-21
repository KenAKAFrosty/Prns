#!/usr/bin/env python3
"""Compare two independent unsigned builds and emit one deterministic candidate."""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path, PurePosixPath
import shutil
import sys
import tarfile
import tempfile

from flasher_reproducibility import (
    REPORT_PATH,
    SEPARATE_ENVELOPES,
    find_generated_envelopes,
    payload_identity,
    payload_manifest,
    sha256,
)
from flasher_website_history import allowed_historical_signatures


MAX_FILES = 100_000
MAX_BYTES = 2 * 1024 * 1024 * 1024
ROOT = Path(__file__).resolve().parents[1]


def extract(archive_path: Path, destination: Path) -> None:
    with tarfile.open(archive_path, "r:gz") as archive:
        members = archive.getmembers()
        if len(members) > MAX_FILES or sum(member.size for member in members) > MAX_BYTES:
            raise ValueError("candidate archive exceeds release extraction limits")
        names: set[str] = set()
        for member in members:
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or member.issym() or member.islnk():
                raise ValueError(f"unsafe candidate archive member: {member.name}")
            if not member.isfile() and not member.isdir():
                raise ValueError(f"unsupported candidate archive member: {member.name}")
            normalized = path.as_posix()
            if normalized in {"", "."}:
                raise ValueError(f"unsafe candidate archive member: {member.name}")
            if normalized in names:
                raise ValueError(f"duplicate candidate archive member: {member.name}")
            names.add(normalized)
        if destination.exists():
            if not destination.is_dir() or any(destination.iterdir()):
                raise ValueError("candidate extraction destination must be an empty directory")
        destination.mkdir(parents=True, exist_ok=True)
        archive.extractall(destination, members=members, filter="data")


def package(root: Path, output: Path) -> None:
    script = ROOT / "scripts" / "package-flasher-candidate.py"
    spec = importlib.util.spec_from_file_location("package_flasher_candidate", script)
    if spec is None or spec.loader is None:
        raise ValueError("could not load deterministic candidate packager")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.package(root, output)


def release_identity(root: Path) -> dict[str, str]:
    manifest = json.loads((root / "flash-manifest.json").read_text(encoding="utf-8"))
    release = manifest.get("release") if isinstance(manifest, dict) else None
    if not isinstance(release, dict):
        raise ValueError("candidate manifest has no release identity")
    version = release.get("version")
    source_commit = release.get("commit")
    if not isinstance(version, str) or not isinstance(source_commit, str):
        raise ValueError("candidate manifest release identity is malformed")
    return {"version": version, "source_commit": source_commit}


def differing_files(
    primary: dict[str, dict[str, int | str]],
    reproduction: dict[str, dict[str, int | str]],
) -> list[str]:
    return sorted(
        relative
        for relative in set(primary) | set(reproduction)
        if primary.get(relative) != reproduction.get(relative)
    )


def files_equal(first: Path, second: Path) -> bool:
    if first.stat().st_size != second.stat().st_size:
        return False
    with first.open("rb") as left, second.open("rb") as right:
        while True:
            left_chunk = left.read(1024 * 1024)
            right_chunk = right.read(1024 * 1024)
            if left_chunk != right_chunk:
                return False
            if not left_chunk:
                return True


def write_sums(root: Path) -> None:
    lines = []
    for relative, identity in payload_manifest(root).items():
        if relative == "SHA256SUMS.txt" or relative.endswith(".minisig"):
            continue
        lines.append(f"{identity['sha256']}  {relative}")
    (root / "SHA256SUMS.txt").write_text(
        "\n".join(lines) + "\n", encoding="utf-8", newline="\n"
    )


def compare(arguments: argparse.Namespace) -> dict:
    primary_archive = arguments.primary.resolve()
    reproduction_archive = arguments.reproduction.resolve()
    if not primary_archive.is_file() or not reproduction_archive.is_file():
        raise ValueError("both independent candidate archives are required")
    with tempfile.TemporaryDirectory(prefix="prns-repro-") as temporary:
        temporary_root = Path(temporary)
        primary_root = temporary_root / "primary"
        reproduction_root = temporary_root / "reproduction"
        extract(primary_archive, primary_root)
        extract(reproduction_archive, reproduction_root)
        primary_files = payload_manifest(primary_root)
        reproduction_files = payload_manifest(reproduction_root)
        differences = differing_files(primary_files, reproduction_files)
        if differences:
            preview = differences[:25]
            raise ValueError(
                f"independent candidate payloads differ ({len(differences)} files): {preview}"
            )
        envelopes = find_generated_envelopes(
            primary_files, allowed=allowed_historical_signatures(primary_root)
        )
        if envelopes:
            raise ValueError(f"unsigned candidate unexpectedly contains signing envelopes: {envelopes}")
        primary_hash = sha256(primary_archive)
        reproduction_hash = sha256(reproduction_archive)
        if primary_hash != reproduction_hash or not files_equal(primary_archive, reproduction_archive):
            raise ValueError("payload files match but deterministic candidate archive bytes differ")
        identity = release_identity(primary_root)
        if release_identity(reproduction_root) != identity:
            raise ValueError("independent candidate manifests have different release identities")
        report = {
            "schema": 1,
            "release": identity,
            "result": "matched",
            "builds": [
                {"name": "primary", "archive_sha256": primary_hash},
                {"name": "reproduction", "archive_sha256": reproduction_hash},
            ],
            "payload": payload_identity(payload_manifest(primary_root, exclude_report=True)),
            "comparison": {"archive_bytes_equal": True, "payload_bytes_equal": True},
            "separate_envelopes": SEPARATE_ENVELOPES,
        }
        encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
        for root in (primary_root, reproduction_root):
            report_path = root / REPORT_PATH
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(encoded, encoding="utf-8", newline="\n")
            write_sums(root)
        first_final = temporary_root / "primary-final.tar.gz"
        second_final = temporary_root / "reproduction-final.tar.gz"
        package(primary_root, first_final)
        package(reproduction_root, second_final)
        if not files_equal(first_final, second_final):
            raise ValueError("deterministic archives diverged after adding identical evidence")
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(first_final, arguments.output)
        arguments.report.parent.mkdir(parents=True, exist_ok=True)
        arguments.report.write_text(encoded, encoding="utf-8", newline="\n")
        return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--primary", type=Path, required=True)
    parser.add_argument("--reproduction", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        report = compare(arguments)
    except (OSError, ValueError, json.JSONDecodeError, tarfile.TarError) as error:
        print(f"flasher candidate reproducibility failed: {error}", file=sys.stderr)
        return 1
    print(
        "independent flasher candidates match: "
        f"{report['payload']['file_count']} files, {report['payload']['total_bytes']} bytes"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
