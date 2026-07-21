"""Shared deterministic payload identities for independent flasher candidate builds."""

from __future__ import annotations

import hashlib
from pathlib import Path

from flasher_website_history import allowed_historical_signatures


REPORT_PATH = "metadata/reproducibility.json"
SEPARATE_ENVELOPES = [
    "Minisign signatures",
    "GitHub/Sigstore attestation bundles",
    "signed physical acceptance",
    "signed release record",
]


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def payload_manifest(root: Path, *, exclude_report: bool = False) -> dict[str, dict[str, int | str]]:
    files: dict[str, dict[str, int | str]] = {}
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"candidate payload contains a symlink: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"candidate payload contains an unsupported entry: {path}")
        relative = path.relative_to(root).as_posix()
        if exclude_report and relative in {REPORT_PATH, "SHA256SUMS.txt"}:
            continue
        files[relative] = {"size": path.stat().st_size, "sha256": sha256(path)}
    return dict(sorted(files.items()))


def payload_identity(files: dict[str, dict[str, int | str]]) -> dict[str, int | str]:
    digest = hashlib.sha256()
    total = 0
    for relative, identity in sorted(files.items()):
        size = identity["size"]
        checksum = identity["sha256"]
        if not isinstance(size, int) or not isinstance(checksum, str):
            raise ValueError("payload file identity is malformed")
        total += size
        digest.update(f"{checksum}  {size}  {relative}\n".encode())
    return {"file_count": len(files), "total_bytes": total, "tree_sha256": digest.hexdigest()}


def find_generated_envelopes(
    files: dict[str, dict[str, int | str]], *, allowed: set[str] | None = None
) -> list[str]:
    allowed = allowed or set()
    found = []
    for relative in files:
        if relative in allowed:
            continue
        name = Path(relative).name
        if (
            relative.endswith(".minisig")
            or name.startswith("acceptance-")
            or name.startswith("release-record-")
            or name.startswith("prns-flasher-attestation-")
        ):
            found.append(relative)
    return found


def validate_report(root: Path, *, version: str, source_commit: str) -> None:
    import json

    report_path = root / REPORT_PATH
    report = json.loads(report_path.read_text(encoding="utf-8"))
    expected_fields = {
        "schema",
        "release",
        "result",
        "builds",
        "payload",
        "comparison",
        "separate_envelopes",
    }
    if not isinstance(report, dict) or set(report) != expected_fields or report.get("schema") != 1:
        raise ValueError("candidate reproducibility evidence has an unsupported shape")
    if report.get("release") != {"version": version, "source_commit": source_commit}:
        raise ValueError("candidate reproducibility evidence has the wrong release identity")
    builds = report.get("builds")
    if not isinstance(builds, list) or len(builds) != 2:
        raise ValueError("candidate reproducibility evidence must contain two independent builds")
    expected_names = ["primary", "reproduction"]
    hashes = []
    for index, build in enumerate(builds):
        if not isinstance(build, dict) or set(build) != {"name", "archive_sha256"}:
            raise ValueError("candidate reproducibility build identity is malformed")
        checksum = build.get("archive_sha256")
        if build.get("name") != expected_names[index] or not isinstance(checksum, str):
            raise ValueError("candidate reproducibility build identity is malformed")
        if len(checksum) != 64 or any(character not in "0123456789abcdef" for character in checksum):
            raise ValueError("candidate reproducibility archive hash is malformed")
        hashes.append(checksum)
    if hashes[0] != hashes[1]:
        raise ValueError("independent candidate archive hashes do not match")
    if report.get("result") != "matched" or report.get("comparison") != {
        "archive_bytes_equal": True,
        "payload_bytes_equal": True,
    }:
        raise ValueError("candidate reproducibility evidence does not record a byte match")
    if report.get("separate_envelopes") != SEPARATE_ENVELOPES:
        raise ValueError("candidate reproducibility evidence obscures separate signing envelopes")
    actual_files = payload_manifest(root, exclude_report=True)
    if find_generated_envelopes(
        actual_files, allowed=allowed_historical_signatures(root)
    ):
        raise ValueError("unsigned reproducibility evidence includes signing-time envelopes")
    if report.get("payload") != payload_identity(actual_files):
        raise ValueError("candidate reproducibility payload identity does not match its files")
