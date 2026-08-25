#!/usr/bin/env python3
"""Compare the public GitHub Release inventory with the exact signed candidate."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from flasher_manifest import FLASH_MANIFEST_SCHEMA
from flasher_public_review import discover_evidence, sha256


SUITE_INVENTORY_ASSETS = ("SHA256SUMS.txt", "SHA256SUMS.txt.minisig")

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


def verify_suite_inventory_signature(assets: Path, public_key: Path) -> None:
    signer = os.environ.get("PRNS_MINISIGN_BIN", "minisign")
    signer_path = shutil.which(signer)
    if signer_path is None:
        raise ValueError(f"configured Minisign executable is unavailable: {signer}")
    verification = subprocess.run(
        [
            signer_path,
            "-Vm",
            str(assets / "SHA256SUMS.txt"),
            "-x",
            str(assets / "SHA256SUMS.txt.minisig"),
            "-p",
            str(public_key),
        ],
        capture_output=True,
    )
    if verification.returncode != 0:
        raise ValueError("suite custody inventory signature verification failed")


def suite_custody_inventory(assets: Path) -> dict[str, str]:
    checksums = assets / "SHA256SUMS.txt"
    if not checksums.is_file():
        raise ValueError("signed suite custody inventory is unavailable")
    inventory: dict[str, str] = {}
    for line in checksums.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        digest, _, name = line.partition("  ")
        name = name.strip()
        if (
            len(digest) != 64
            or any(character not in "0123456789abcdef" for character in digest)
            or not name
        ):
            raise ValueError("signed suite custody inventory entry is invalid")
        if name in inventory:
            raise ValueError(f"signed suite custody inventory repeats {name}")
        inventory[name] = digest
    if not inventory:
        raise ValueError("signed suite custody inventory is empty")
    return inventory


def expected_candidate_assets(candidate: Path, version: str) -> dict[str, Path]:
    manifest = json.loads((candidate / "flash-manifest.json").read_text(encoding="utf-8"))
    schema = manifest.get("schema") if isinstance(manifest, dict) else None
    if (
        not isinstance(schema, int)
        or isinstance(schema, bool)
        or schema < 1
        or schema > FLASH_MANIFEST_SCHEMA
    ):
        raise ValueError("signed candidate manifest schema is unsupported")
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
    if schema >= 3:
        sources["flasher_manifest.py"] = (
            candidate / "qualification" / "flasher_manifest.py"
        )
    hotfix_metadata = candidate / "metadata" / "hotfix.json"
    hotfix_helper = candidate / "qualification" / "flasher_hotfix.py"
    if hotfix_helper.is_file():
        sources["flasher_hotfix.py"] = hotfix_helper
    elif hotfix_metadata.is_file():
        raise ValueError("signed hotfix candidate release asset is missing: flasher_hotfix.py")
    if hotfix_metadata.is_file():
        sources[f"hotfix-inheritance-v{version}.json"] = (
            hotfix_metadata
        )
        sources[f"hotfix-spec-v{version}.json"] = (
            candidate / "qualification" / "hotfix.json"
        )
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


def required_custody_assets(candidate: Path, version: str) -> set[str]:
    names = {
        f"prns-flasher-candidate-v{version}-signed.tar.gz",
        f"prns-flasher-candidate-run-v{version}.json",
        f"prns-flasher-attestation-v{version}.json",
        f"prns-flasher-attestation-v{version}.metadata.json",
        f"acceptance-v{version}.json",
        f"acceptance-v{version}.json.minisig",
        f"qualification-evidence-v{version}.tar.gz",
        f"flasher-release-record-v{version}.json",
        f"flasher-release-record-v{version}.json.minisig",
    }
    if not (candidate / "metadata" / "hotfix.json").is_file():
        names.update(
            {
                f"release-record-v{version}.json",
                f"release-record-v{version}.json.minisig",
            }
        )
    return names


def verify(
    candidate: Path, assets: Path, version: str, remote_inventory: Path | None = None
) -> None:
    candidate_sources = expected_candidate_assets(candidate, version)
    manifest_path = candidate / "flash-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    release = manifest.get("release")
    if not isinstance(release, dict):
        raise ValueError("signed candidate release identity is unavailable")
    custody_names = required_custody_assets(candidate, version)
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
        workflow_run_id=None,
        signed_candidate_sha256=sha256(signed_bundle),
        manifest_sha256=sha256(manifest_path),
    )
    expected_names = (
        set(candidate_sources)
        | custody_names
        | {path.name for path in public_review_assets}
    )
    missing = expected_names - actual_names
    if missing:
        raise ValueError(
            "GitHub Release asset inventory is missing signed release assets: "
            f"{sorted(missing)}"
        )
    suite_inventory_replaces_candidate_copy = any(
        not files_equal(candidate_sources[name], assets / name)
        for name in SUITE_INVENTORY_ASSETS
    )
    inventory = None
    if suite_inventory_replaces_candidate_copy:
        verify_suite_inventory_signature(assets, candidate_sources["minisign.pub"])
        inventory = suite_custody_inventory(assets)
        contradictions = sorted(
            name
            for name, source in candidate_sources.items()
            if name not in SUITE_INVENTORY_ASSETS
            and name in inventory
            and inventory[name] != sha256(source)
        )
        if contradictions:
            raise ValueError(
                "signed suite custody inventory contradicts the signed candidate: "
                f"{contradictions}"
            )
    extras = actual_names - expected_names
    if extras:
        if inventory is None:
            inventory = suite_custody_inventory(assets)
        unaccounted = sorted(
            name for name in extras if inventory.get(name) != sha256(assets / name)
        )
        if unaccounted:
            raise ValueError(
                "GitHub Release assets are outside both the signed candidate and the "
                f"signed suite custody inventory: {unaccounted}"
            )
    for name, source in candidate_sources.items():
        if suite_inventory_replaces_candidate_copy and name in SUITE_INVENTORY_ASSETS:
            continue
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
