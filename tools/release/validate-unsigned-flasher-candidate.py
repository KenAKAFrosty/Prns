#!/usr/bin/env python3
"""Verify an unsigned candidate's identity and complete checksum envelope before signing."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys

from flasher_build_metadata import validate_metadata
from flasher_browser_test_trust import find_browser_test_trust_leaks
from flasher_hotfix import verify_candidate as verify_hotfix_candidate
from flasher_reproducibility import validate_report as validate_reproducibility_report
from flasher_sparse_sizes import build_report as build_sparse_size_report
from flasher_website_history import allowed_historical_signatures, validate_candidate_history
from source_snapshot import verify_source_snapshot
from flasher_manifest import (
    FLASH_MANIFEST_SCHEMA,
    target_artifacts,
    validate_nrf_serial_dfu_recovery_artifact,
    validate_uf2_artifact,
)


CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
SHIPPING_BOARDS = {
    "heltec-v4",
    "heltec-v4-r8",
    "t-beam-supreme",
    "xiao-esp32-c6",
    "t-echo",
    "t114",
    "t096",
    "t1000-e",
}
REQUIRED_RELEASE_FILES = (
    "VERSION",
    "flash-manifest.json",
    "minisign.pub",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "THIRD_PARTY_NOTICES.md",
    "metadata/build.json",
    "metadata/source.json",
    "metadata/source-capabilities.json",
    "metadata/sparse-sizes.json",
    "metadata/reproducibility.json",
    "metadata/release-history.json",
    "audit/release-audit-evidence.md",
    "qualification/QUALIFICATION.md",
    "qualification/create-flasher-acceptance.py",
    "qualification/validate-flasher-acceptance.py",
    "qualification/flasher_acceptance_contract.py",
    "qualification/flasher_manifest.py",
    "qualification/flasher_tester_roster.py",
    "qualification/package-flasher-qualification-evidence.py",
    "qualification/serve-flasher-candidate.py",
    "qualification/verify-flasher-candidate-files.py",
    "qualification/validate-flasher-tester-roster.py",
    "qualification/tester-roster.json",
    "website/index.html",
    "website/404.html",
    "website/assets/flasher/prns-flash.js",
    "website/assets/flasher/nrf-dfu/prns_nrf_dfu_core.js",
    "website/assets/flasher/nrf-dfu/prns_nrf_dfu_core.d.ts",
    "website/assets/flasher/nrf-dfu/prns_nrf_dfu_core_bg.wasm",
    "website/assets/flasher/nrf-dfu/prns_nrf_dfu_core_bg.wasm.d.ts",
    "website/source.zip",
    "website/source.zip.sha256",
    "website/browser-node-playground-console/pkg/prns_wasm_bg.wasm",
)
FORBIDDEN_PRODUCTION_REFERENCES = (
    b"esp-web-install-button",
    b"esp-web-tools",
    b"unpkg.com",
    b"playwright",
    b"axe-core",
)


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
    allowed_signatures = allowed_historical_signatures(root)
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"candidate cannot contain symlink {path}")
        if not path.is_file():
            continue
        relative = path.relative_to(root).as_posix()
        if relative == "SHA256SUMS.txt" or relative in allowed_signatures:
            continue
        files.add(relative)
    return files


def verify_required_release_files(root: Path) -> None:
    for relative in REQUIRED_RELEASE_FILES:
        path = safe_path(root, relative)
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"candidate required release file is missing or empty: {relative}")
    for mutable in (
        root / "website" / "flash-manifest.json",
        root / "website" / "firmware",
        root / "website" / "assets" / "firmware",
    ):
        if mutable.exists():
            raise ValueError(f"candidate contains a mutable hosted release path: {mutable}")


def verify_qualification_kit(root: Path, roster_version: str, tester_roster: Path) -> None:
    repository = Path(__file__).resolve().parents[2]
    release_tools = repository / "tools" / "release"
    exact_sources = {
        "qualification/QUALIFICATION.md": repository / "release" / "acceptance" / "QUALIFICATION.md",
        "qualification/create-flasher-acceptance.py": release_tools / "create-flasher-acceptance.py",
        "qualification/validate-flasher-acceptance.py": release_tools / "validate-flasher-acceptance.py",
        "qualification/flasher_acceptance_contract.py": release_tools
        / "flasher_acceptance_contract.py",
        "qualification/flasher_manifest.py": release_tools / "flasher_manifest.py",
        "qualification/flasher_tester_roster.py": release_tools / "flasher_tester_roster.py",
        "qualification/flasher_hotfix.py": release_tools / "flasher_hotfix.py",
        "qualification/package-flasher-qualification-evidence.py": release_tools
        / "package-flasher-qualification-evidence.py",
        "qualification/serve-flasher-candidate.py": release_tools / "serve-flasher-candidate.py",
        "qualification/verify-flasher-candidate-files.py": release_tools
        / "verify-flasher-candidate-files.py",
        "qualification/validate-flasher-tester-roster.py": release_tools
        / "validate-flasher-tester-roster.py",
        "qualification/tester-roster.json": tester_roster,
    }
    for relative, source in exact_sources.items():
        candidate_path = safe_path(root, relative)
        if not source.is_file() or candidate_path.read_bytes() != source.read_bytes():
            raise ValueError(
                f"candidate qualification file differs from its reviewed source: {relative}"
            )

    validation = subprocess.run(
        [
            sys.executable,
            str(release_tools / "validate-flasher-tester-roster.py"),
            "--roster",
            str(root / "qualification" / "tester-roster.json"),
            "--version",
            roster_version,
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    if validation.returncode != 0:
        detail = validation.stderr.strip() or validation.stdout.strip()
        raise ValueError(f"candidate tester roster is invalid: {detail}")


def verify_production_website(root: Path) -> None:
    bundle = (root / "website" / "assets" / "flasher" / "prns-flash.js").read_bytes().lower()
    for forbidden in FORBIDDEN_PRODUCTION_REFERENCES:
        if forbidden in bundle:
            raise ValueError(
                f"hosted flasher bundle contains forbidden production reference {forbidden.decode()}"
            )

    fixture_key_path = (
        Path(__file__).resolve().parents[2]
        / "docs"
        / "website"
        / "web-flasher"
        / "browser"
        / "fixtures"
        / "signed-candidate"
        / "minisign.pub"
    )
    source_archive_path = root / "website" / "source.zip"
    leaks = find_browser_test_trust_leaks(
        (root / "website",),
        fixture_key_path,
        root / "minisign.pub",
        source_archive_path,
    )
    if leaks:
        raise ValueError(
            f"hosted website contains browser-test trust material: "
            f"{leaks[0].path.relative_to(root).as_posix()}"
        )


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
    verify_required_release_files(root)
    validate_candidate_history(root)
    allowed_signatures = allowed_historical_signatures(root)
    signatures = {
        path.relative_to(root).as_posix() for path in root.rglob("*.minisig")
    }
    unexpected_signatures = sorted(signatures - allowed_signatures)
    if unexpected_signatures:
        raise ValueError(
            f"unsigned candidate contains current or untracked signatures: {unexpected_signatures}"
        )
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
    if not version or version.lower() == "next":
        raise ValueError("candidate VERSION is not publishable")
    if any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+" for character in version):
        raise ValueError("candidate VERSION is not path-safe")
    hotfix = verify_hotfix_candidate(arguments.source_repository.resolve(), root)
    if hotfix is None and version != repository_version:
        raise ValueError("ordinary candidate VERSION differs from repository VERSION")
    if hotfix is not None and hotfix.roster_version != repository_version:
        raise ValueError("hotfix prefix differs from repository VERSION")
    verify_qualification_kit(root, repository_version, arguments.tester_roster.resolve())
    if not (
        len(arguments.expected_commit) == 40
        and all(character in "0123456789abcdef" for character in arguments.expected_commit)
    ):
        raise ValueError("expected source commit must be a lowercase full Git commit")
    verify_source_snapshot(
        repository=arguments.source_repository,
        commit=arguments.expected_commit,
        version=repository_version,
        archive=root / "website" / "source.zip",
        checksum=root / "website" / "source.zip.sha256",
        metadata=root / "metadata" / "source.json",
    )
    verify_production_website(root)
    source_metadata = json.loads(
        (root / "metadata" / "source.json").read_text(encoding="utf-8")
    )
    source_archive = (root / "website" / "source.zip").read_bytes()
    browser_wasm = (
        root
        / "website"
        / "browser-node-playground-console"
        / "pkg"
        / "prns_wasm_bg.wasm"
    ).read_bytes()
    for marker in (
        source_metadata["sha256"].encode(),
        arguments.expected_commit[:12].encode(),
        b"/file/source.zip",
        b"/file/source.zip.sha256",
    ):
        if marker not in browser_wasm:
            raise ValueError(
                "source-enabled browser playground does not carry the candidate source identity"
            )

    manifest_path = root / "flash-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    release = manifest.get("release")
    signing = manifest.get("signing")
    if manifest.get("schema") != FLASH_MANIFEST_SCHEMA or not isinstance(release, dict) or not isinstance(signing, dict):
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
        raise ValueError("candidate manifest does not contain exactly the shipping board set")
    immutable_root = root / "website" / "releases" / version
    for target in targets:
        if not isinstance(target, dict):
            raise ValueError("candidate manifest contains a malformed target")
        parts = target_artifacts(target)
        board_slug = target.get("board_slug")
        source = target.get("source")
        if source is not None:
            raise ValueError(f"embedded target {board_slug} must not carry source metadata")
        payloads_by_kind = {}
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
            payload = artifact.read_bytes()
            kind = part.get("kind")
            if isinstance(kind, str):
                payloads_by_kind[kind] = payload
            if not hosted.is_file() or hosted.read_bytes() != payload:
                raise ValueError(f"hosted firmware part differs from candidate payload: {relative}")
            if target.get("transport") == "uf2-mass-storage":
                validate_uf2_artifact(part, payload)
            if part.get("kind") in {"application", "dfu-application"} and source_archive in payload:
                raise ValueError(
                    f"embedded target {board_slug} must not embed source.zip"
                )
        if target.get("transport") == "nrf-serial-dfu":
            application = payloads_by_kind.get("dfu-application")
            recovery = payloads_by_kind.get("uf2")
            if application is None or recovery is None:
                raise ValueError("Nordic serial DFU target is missing bound artifacts")
            validate_nrf_serial_dfu_recovery_artifact(target, application, recovery)

    capability_metadata = json.loads(
        (root / "metadata" / "source-capabilities.json").read_text(encoding="utf-8")
    )
    if (
        capability_metadata.get("schema") != 1
        or capability_metadata.get("version") != version
        or capability_metadata.get("commit") != arguments.expected_commit
    ):
        raise ValueError("source capability metadata has the wrong release identity")
    capabilities = capability_metadata.get("targets")
    if not isinstance(capabilities, list) or len(capabilities) != len(SHIPPING_BOARDS):
        raise ValueError("source capability metadata must cover every shipping board")
    capability_by_board = {
        item.get("board_slug"): item for item in capabilities if isinstance(item, dict)
    }
    if set(capability_by_board) != SHIPPING_BOARDS:
        raise ValueError("source capability metadata has a malformed board set")
    for board_slug, capability in capability_by_board.items():
        if capability.get("nominally_capable") is not False:
            raise ValueError(
                f"{board_slug} source capability metadata disagrees with the board catalog"
            )
        if (
            capability.get("status") != "absent"
            or capability.get("source") is not None
            or capability.get("reserve_bytes") is not None
        ):
            raise ValueError(f"{board_slug} must explicitly record source capability as absent")

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
    if not isinstance(metadata, dict):
        raise ValueError("candidate build metadata must be an object")
    validate_metadata(metadata, commit=arguments.expected_commit)
    sparse_path = root / "metadata" / "sparse-sizes.json"
    sparse_report = json.loads(sparse_path.read_text(encoding="utf-8"))
    if sparse_report != build_sparse_size_report(manifest):
        raise ValueError("candidate sparse-size evidence differs from its manifest")
    validate_reproducibility_report(
        root, version=version, source_commit=arguments.expected_commit
    )
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
    parser.add_argument("--tester-roster", type=Path, required=True)
    parser.add_argument(
        "--source-repository",
        type=Path,
        default=Path(__file__).resolve().parents[2],
    )
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
