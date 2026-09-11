from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from validation.hardening.embedded_execution import ProcessObservation
from validation.hardening.embedded_platform.contract import EmulatorKind, ROOT, Platform
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


@dataclass(frozen=True)
class EmulatorEvidence:
    kind: EmulatorKind
    identity: str
    executable_sha256: str
    details: tuple[str, ...]


def directory() -> Path:
    artifact_root = Path(
        os.environ.get("PRNS_VALIDATION_ARTIFACT_ROOT", ROOT / "validation-artifacts")
    ).resolve()
    suite = os.environ.get("PRNS_VALIDATION_SUITE", "embedded-platform-development")
    artifact_directory = Path(
        os.environ.get(
            "PRNS_VALIDATION_ARTIFACT_DIR",
            artifact_root / "results" / suite,
        )
    ).resolve()
    if artifact_root != artifact_directory and artifact_root not in artifact_directory.parents:
        raise EmbeddedPlatformError(
            "embedded platform artifact directory is outside its artifact root"
        )
    artifact_directory.mkdir(parents=True, exist_ok=True)
    return artifact_directory


def clear(platform: Platform, artifact_directory: Path) -> None:
    for suffix in (
        ".assurance.json",
        ".config",
        ".elf",
        ".flash.bin",
        ".log",
        ".repl",
        ".resc",
        ".transcript",
    ):
        path = artifact_directory / f"{platform.identifier}{suffix}"
        if path.is_file() or path.is_symlink():
            path.unlink()


def render_log(
    platform: Platform,
    observations: tuple[ProcessObservation, ...],
    cargo_version: str,
    rustc_version: str,
    linker_identity: str,
    emulator: EmulatorEvidence,
) -> bytes:
    lines = [
        f"platform={platform.identifier}",
        f"architecture={platform.architecture}",
        f"memory-profile={platform.memory_profile}",
        f"scenario={platform.scenario}",
        f"milestone={platform.milestone.value}",
        f"runner={platform.runner}",
        f"cargo={cargo_version}",
        f"rustc={rustc_version}",
        f"linker={linker_identity}",
        f"emulator-kind={emulator.kind.value}",
        f"emulator={emulator.identity}",
        f"emulator-executable-sha256={emulator.executable_sha256}",
        *emulator.details,
    ]
    body = bytearray(("\n".join(lines) + "\n").encode())
    for observation in observations:
        body.extend(f"\ncommand={' '.join(observation.command)}\n".encode())
        body.extend(f"exit-reason={observation.reason.value}\n".encode())
        body.extend(f"exit-status={observation.returncode}\n".encode())
        body.extend(b"stdout:\n")
        body.extend(observation.stdout)
        body.extend(b"\nstderr:\n")
        body.extend(observation.stderr)
        body.extend(b"\n")
    return bytes(body)
