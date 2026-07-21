#!/usr/bin/env python3
"""Compare the public GitHub Release inventory with the exact signed candidate."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys


CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}


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


def expected_candidate_assets(candidate: Path, version: str) -> dict[str, Path]:
    manifest = json.loads((candidate / "flash-manifest.json").read_text(encoding="utf-8"))
    release = manifest.get("release") if isinstance(manifest, dict) else None
    if not isinstance(release, dict) or release.get("version") != version:
        raise ValueError("signed candidate manifest differs from the release version")
    channel = release.get("channel")
    if channel != "stable":
        raise ValueError("public promotion requires the signed stable channel candidate")
    sources = {
        "SHA256SUMS.txt": candidate / "SHA256SUMS.txt",
        "SHA256SUMS.txt.minisig": candidate / "SHA256SUMS.txt.minisig",
        "flash-manifest.json": candidate / "flash-manifest.json",
        "flash-manifest.json.minisig": candidate / "flash-manifest.json.minisig",
        "stable.json": candidate / "channels" / "stable.json",
        "stable.json.minisig": candidate / "channels" / "stable.json.minisig",
        "minisign.pub": candidate / "minisign.pub",
        "install.sh": candidate / "cli" / "install.sh",
        "install.ps1": candidate / "cli" / "install.ps1",
        "README.md": candidate / "cli" / "README.md",
    }
    for target, extension in CLI_TARGETS.items():
        name = f"hopspot-flash-{version}-{target}{extension}"
        sources[name] = candidate / "cli" / name
    for name, path in sources.items():
        if not path.is_file():
            raise ValueError(f"signed candidate release asset is missing: {name}")
    return sources


def verify(candidate: Path, assets: Path, version: str) -> None:
    candidate_sources = expected_candidate_assets(candidate, version)
    custody_names = {
        f"prns-flasher-candidate-v{version}-signed.tar.gz",
        f"prns-flasher-candidate-run-v{version}.json",
        f"prns-flasher-attestation-v{version}.json",
        f"prns-flasher-attestation-v{version}.metadata.json",
        f"acceptance-v{version}.json",
        f"acceptance-v{version}.json.minisig",
        f"release-record-v{version}.json",
        f"release-record-v{version}.json.minisig",
    }
    if not assets.is_dir():
        raise ValueError("downloaded GitHub Release asset directory is unavailable")
    entries = list(assets.iterdir())
    if any(not entry.is_file() or entry.is_symlink() for entry in entries):
        raise ValueError("downloaded GitHub Release assets contain a non-file entry")
    actual_names = {entry.name for entry in entries}
    if len(actual_names) != len(entries):
        raise ValueError("downloaded GitHub Release asset names are ambiguous")
    expected_names = set(candidate_sources) | custody_names
    if actual_names != expected_names:
        raise ValueError(
            "GitHub Release asset inventory differs from the signed release; "
            f"missing={sorted(expected_names - actual_names)}, "
            f"unexpected={sorted(actual_names - expected_names)}"
        )
    for name, source in candidate_sources.items():
        if not files_equal(source, assets / name):
            raise ValueError(f"GitHub Release asset bytes differ from the candidate: {name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--version", required=True)
    arguments = parser.parse_args()
    try:
        verify(arguments.candidate, arguments.assets, arguments.version)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"release asset verification failed: {error}", file=sys.stderr)
        return 1
    print(f"verified exact GitHub Release asset inventory for {arguments.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
