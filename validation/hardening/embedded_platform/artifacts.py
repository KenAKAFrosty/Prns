from __future__ import annotations

import os
from pathlib import Path

from validation.hardening.embedded_execution import ProcessObservation
from validation.hardening.embedded_platform.contract import ROOT, Platform
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


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
    emulator_identity: str,
    emulator_executable_sha256: str,
    platform_description: Path,
    platform_description_sha256: str,
    effective_platform_description: Path,
    effective_platform_description_sha256: str,
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
        f"emulator={emulator_identity}",
        f"emulator-executable-sha256={emulator_executable_sha256}",
        f"emulator-source={platform.emulator.source_repository}",
        f"emulator-source-revision={platform.emulator.source_revision}",
        f"platform-description={platform_description}",
        f"platform-description-sha256={platform_description_sha256}",
        f"effective-platform-description={effective_platform_description}",
        "effective-platform-description-sha256="
        f"{effective_platform_description_sha256}",
    ]
    for package in platform.emulator.packages:
        lines.extend(
            (
                f"emulator-package-{package.host.value}={package.source_url}",
                f"emulator-package-{package.host.value}-sha256={package.source_sha256}",
            )
        )
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
