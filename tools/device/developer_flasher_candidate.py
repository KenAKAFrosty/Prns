from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import sys


RELEASE_TOOLS = Path(__file__).resolve().parents[1] / "release"
if str(RELEASE_TOOLS) not in sys.path:
    sys.path.insert(0, str(RELEASE_TOOLS))

from flasher_manifest import (
    require_schema,
    target_artifacts,
    validate_nrf_serial_dfu_recovery_artifact,
    validate_uf2_artifact,
)


class DeveloperCandidateError(RuntimeError):
    pass


@dataclass(frozen=True)
class ExpectedTarget:
    board_slug: str
    transport: str


@dataclass(frozen=True)
class ValidatedArtifact:
    path: PurePosixPath
    payload: bytes


@dataclass(frozen=True)
class ValidatedTarget:
    board_slug: str
    transport: str
    artifacts: tuple[ValidatedArtifact, ...]


@dataclass(frozen=True)
class ValidatedCandidate:
    version: str
    channel: str
    commit: str
    key_id: str
    targets: tuple[ValidatedTarget, ...]


def safe_artifact_path(candidate: Path, wire_path: object) -> tuple[PurePosixPath, Path]:
    if not isinstance(wire_path, str):
        raise DeveloperCandidateError("manifest contains a non-string artifact path")
    relative = PurePosixPath(wire_path)
    if (
        relative.is_absolute()
        or not relative.parts
        or any(part in {"", ".", ".."} for part in relative.parts)
    ):
        raise DeveloperCandidateError(f"manifest contains unsafe artifact path: {wire_path!r}")
    root = candidate.resolve(strict=True)
    path = root.joinpath(*relative.parts)
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(root)
    except (FileNotFoundError, RuntimeError, ValueError) as error:
        raise DeveloperCandidateError(f"manifest artifact is unavailable: {wire_path}") from error
    if resolved != path.absolute():
        raise DeveloperCandidateError(f"manifest artifact path contains a link: {wire_path}")
    metadata = path.stat(follow_symlinks=False)
    if not stat.S_ISREG(metadata.st_mode):
        raise DeveloperCandidateError(f"manifest artifact is not a regular file: {wire_path}")
    return relative, path


def read_stable_artifact(path: Path, wire_path: PurePosixPath) -> bytes:
    before = path.stat(follow_symlinks=False)
    with path.open("rb") as source:
        payload = source.read()
    after = path.stat(follow_symlinks=False)
    identity = lambda value: (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )
    if identity(before) != identity(after):
        raise DeveloperCandidateError(f"manifest artifact changed while validating: {wire_path}")
    return payload


def validate_artifact(
    candidate: Path,
    artifact: object,
    transport: str,
    version: str,
    source_digest: str,
) -> ValidatedArtifact:
    if not isinstance(artifact, dict):
        raise DeveloperCandidateError("assembled manifest contains an invalid artifact")
    relative, path = safe_artifact_path(candidate, artifact.get("path"))
    payload = read_stable_artifact(path, relative)
    expected_size = artifact.get("size")
    expected_hash = artifact.get("sha256")
    if (
        isinstance(expected_size, bool)
        or not isinstance(expected_size, int)
        or expected_size != len(payload)
        or not isinstance(expected_hash, str)
        or expected_hash != hashlib.sha256(payload).hexdigest()
    ):
        raise DeveloperCandidateError(
            f"assembled manifest hash or size disagrees with {relative.as_posix()!r}"
        )
    if transport == "uf2-mass-storage":
        try:
            validate_uf2_artifact(artifact, payload)
        except (KeyError, TypeError, ValueError) as error:
            raise DeveloperCandidateError(
                f"assembled manifest UF2 evidence is invalid for {relative.as_posix()!r}: {error}"
            ) from error
    if transport == "esp-serial" and artifact.get("kind") == "application":
        embedded_identity = f"version={version} source={source_digest}".encode("ascii")
        if embedded_identity not in payload:
            raise DeveloperCandidateError(
                "ESP application does not embed the signed developer version and source digest"
            )
    return ValidatedArtifact(relative, payload)


def validate_candidate(
    candidate: Path,
    manifest_path: Path,
    version: str,
    commit: str,
    source_digest: str,
    key_id: str,
    expected_targets: tuple[ExpectedTarget, ...],
) -> ValidatedCandidate:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise DeveloperCandidateError(f"assembled manifest is unreadable: {error}") from error
    if not isinstance(manifest, dict):
        raise DeveloperCandidateError("assembled manifest must be a JSON object")
    try:
        require_schema(manifest)
    except ValueError as error:
        raise DeveloperCandidateError(str(error)) from error
    release = manifest.get("release")
    signing = manifest.get("signing")
    targets = manifest.get("targets")
    if (
        release != {"version": version, "channel": "preview", "commit": commit}
        or signing != {"key_id": key_id}
        or not isinstance(targets, list)
    ):
        raise DeveloperCandidateError("assembled manifest release identity is invalid")
    actual_boards = tuple(
        target.get("board_slug") if isinstance(target, dict) else None for target in targets
    )
    expected_boards = tuple(target.board_slug for target in expected_targets)
    if actual_boards != expected_boards:
        raise DeveloperCandidateError("assembled manifest target set is not the exact selection")
    if len(set(actual_boards)) != len(actual_boards):
        raise DeveloperCandidateError("assembled manifest target set contains duplicates")
    seen_paths: set[PurePosixPath] = set()
    validated_targets = []
    for target, expected in zip(targets, expected_targets):
        transport = target.get("transport")
        if transport != expected.transport:
            raise DeveloperCandidateError(
                f"assembled manifest transport disagrees with {expected.board_slug}"
            )
        try:
            artifacts = target_artifacts(target)
        except ValueError as error:
            raise DeveloperCandidateError(str(error)) from error
        validated_artifacts = tuple(
            validate_artifact(candidate, artifact, transport, version, source_digest)
            for artifact in artifacts
        )
        if transport == "nrf-serial-dfu":
            try:
                validate_nrf_serial_dfu_recovery_artifact(
                    target,
                    validated_artifacts[0].payload,
                    validated_artifacts[2].payload,
                )
            except (IndexError, KeyError, TypeError, ValueError) as error:
                raise DeveloperCandidateError(
                    f"assembled manifest Nordic recovery evidence is invalid for "
                    f"{expected.board_slug}: {error}"
                ) from error
        paths = tuple(artifact.path for artifact in validated_artifacts)
        duplicates = seen_paths.intersection(paths)
        if len(set(paths)) != len(paths) or duplicates:
            duplicate = next(path for path in paths if paths.count(path) > 1 or path in duplicates)
            raise DeveloperCandidateError(
                f"assembled manifest repeats artifact path {duplicate.as_posix()!r}"
            )
        seen_paths.update(paths)
        if transport == "esp-serial" and not any(
            artifact.get("kind") == "application" for artifact in artifacts
        ):
            raise DeveloperCandidateError(
                f"assembled manifest ESP target has no application image: {expected.board_slug}"
            )
        validated_targets.append(
            ValidatedTarget(expected.board_slug, transport, validated_artifacts)
        )
    return ValidatedCandidate(
        version,
        "preview",
        commit,
        key_id,
        tuple(validated_targets),
    )
