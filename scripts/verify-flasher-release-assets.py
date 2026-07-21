#!/usr/bin/env python3
"""Compare the public GitHub Release inventory with the exact signed candidate."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from flasher_public_review import discover_evidence, sha256


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
        "QUALIFICATION.md": candidate / "qualification" / "QUALIFICATION.md",
        "create-flasher-acceptance.py": candidate
        / "qualification"
        / "create-flasher-acceptance.py",
        "validate-flasher-acceptance.py": candidate
        / "qualification"
        / "validate-flasher-acceptance.py",
        "flasher_acceptance_contract.py": candidate
        / "qualification"
        / "flasher_acceptance_contract.py",
        "flasher_tester_roster.py": candidate / "qualification" / "flasher_tester_roster.py",
        "package-flasher-qualification-evidence.py": candidate
        / "qualification"
        / "package-flasher-qualification-evidence.py",
        "serve-flasher-candidate.py": candidate / "qualification" / "serve-flasher-candidate.py",
        "verify-flasher-candidate-files.py": candidate
        / "qualification"
        / "verify-flasher-candidate-files.py",
        "validate-flasher-tester-roster.py": candidate
        / "qualification"
        / "validate-flasher-tester-roster.py",
        "tester-roster.json": candidate / "qualification" / "tester-roster.json",
        "release-audit-evidence.md": candidate / "audit" / "release-audit-evidence.md",
        "build.json": candidate / "metadata" / "build.json",
        "sparse-sizes.json": candidate / "metadata" / "sparse-sizes.json",
        "reproducibility.json": candidate / "metadata" / "reproducibility.json",
        "release-history.json": candidate / "metadata" / "release-history.json",
    }
    for target, extension in CLI_TARGETS.items():
        name = f"hopspot-flash-{version}-{target}{extension}"
        sources[name] = candidate / "cli" / name
    for name, path in sources.items():
        if not path.is_file():
            raise ValueError(f"signed candidate release asset is missing: {name}")
    return sources


def verify_remote_inventory(assets: Path, inventory_path: Path) -> None:
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    if not isinstance(inventory, list):
        raise ValueError("GitHub Release asset inventory must be a JSON array")
    expected = {}
    for item in inventory:
        if not isinstance(item, dict) or set(item) != {"name", "size", "digest"}:
            raise ValueError("GitHub Release asset inventory entry is malformed")
        name = item.get("name")
        size = item.get("size")
        digest = item.get("digest")
        if (
            not isinstance(name, str)
            or not name
            or "/" in name
            or "\\" in name
            or name in expected
            or not isinstance(size, int)
            or isinstance(size, bool)
            or size < 0
            or not isinstance(digest, str)
            or not digest.startswith("sha256:")
        ):
            raise ValueError("GitHub Release asset inventory entry is invalid")
        checksum = digest.removeprefix("sha256:")
        if len(checksum) != 64 or any(
            character not in "0123456789abcdef" for character in checksum
        ):
            raise ValueError("GitHub Release asset inventory digest is invalid")
        expected[name] = {"size": size, "sha256": checksum}
    local = {path.name: path for path in assets.iterdir()}
    if set(local) != set(expected):
        raise ValueError("downloaded assets differ from the GitHub Release inventory")
    for name, identity in expected.items():
        path = local[name]
        if path.stat().st_size != identity["size"] or sha256(path) != identity["sha256"]:
            raise ValueError(f"downloaded asset bytes differ from GitHub digest: {name}")


def verify(
    candidate: Path, assets: Path, version: str, remote_inventory: Path | None = None
) -> None:
    candidate_sources = expected_candidate_assets(candidate, version)
    manifest_path = candidate / "flash-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    release = manifest.get("release")
    if not isinstance(release, dict):
        raise ValueError("signed candidate release identity is unavailable")
    custody_names = {
        f"prns-flasher-candidate-v{version}-signed.tar.gz",
        f"prns-flasher-candidate-run-v{version}.json",
        f"prns-flasher-attestation-v{version}.json",
        f"prns-flasher-attestation-v{version}.metadata.json",
        f"acceptance-v{version}.json",
        f"acceptance-v{version}.json.minisig",
        f"qualification-evidence-v{version}.tar.gz",
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
    attestation_metadata_path = (
        assets / f"prns-flasher-attestation-v{version}.metadata.json"
    )
    attestation_metadata = json.loads(
        attestation_metadata_path.read_text(encoding="utf-8")
    )
    if not isinstance(attestation_metadata, dict):
        raise ValueError("attestation metadata must be a JSON object")
    repository = attestation_metadata.get("repository")
    workflow_run_id = attestation_metadata.get("workflow_run_id")
    source_commit = release.get("commit")
    if not isinstance(repository, str) or not repository:
        raise ValueError("attestation metadata repository is unavailable")
    if (
        not isinstance(workflow_run_id, int)
        or isinstance(workflow_run_id, bool)
        or workflow_run_id <= 0
    ):
        raise ValueError("attestation metadata workflow run ID is unavailable")
    if not isinstance(source_commit, str):
        raise ValueError("signed candidate source commit is unavailable")
    signed_bundle = assets / f"prns-flasher-candidate-v{version}-signed.tar.gz"
    public_review_assets = discover_evidence(
        assets,
        repository=repository,
        version=version,
        source_commit=source_commit,
        workflow_run_id=workflow_run_id,
        signed_candidate_sha256=sha256(signed_bundle),
        manifest_sha256=sha256(manifest_path),
    )
    expected_names = (
        set(candidate_sources)
        | custody_names
        | {path.name for path in public_review_assets}
    )
    if actual_names != expected_names:
        raise ValueError(
            "GitHub Release asset inventory differs from the signed release; "
            f"missing={sorted(expected_names - actual_names)}, "
            f"unexpected={sorted(actual_names - expected_names)}"
        )
    for name, source in candidate_sources.items():
        if not files_equal(source, assets / name):
            raise ValueError(f"GitHub Release asset bytes differ from the candidate: {name}")
    if remote_inventory is not None:
        verify_remote_inventory(assets, remote_inventory)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--remote-inventory", type=Path)
    arguments = parser.parse_args()
    try:
        verify(
            arguments.candidate,
            arguments.assets,
            arguments.version,
            arguments.remote_inventory,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"release asset verification failed: {error}", file=sys.stderr)
        return 1
    print(f"verified exact GitHub Release asset inventory for {arguments.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
