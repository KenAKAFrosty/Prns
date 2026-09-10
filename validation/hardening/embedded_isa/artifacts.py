from __future__ import annotations

import os
from pathlib import Path

from validation.hardening.embedded_isa.contract import ROOT, Architecture
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.process import ProcessObservation


def directory() -> Path:
    artifact_root = Path(
        os.environ.get("PRNS_VALIDATION_ARTIFACT_ROOT", ROOT / "validation-artifacts")
    ).resolve()
    suite = os.environ.get("PRNS_VALIDATION_SUITE", "embedded-isa-development")
    artifact_directory = Path(
        os.environ.get(
            "PRNS_VALIDATION_ARTIFACT_DIR",
            artifact_root / "results" / suite,
        )
    ).resolve()
    if artifact_root != artifact_directory and artifact_root not in artifact_directory.parents:
        raise EmbeddedIsaError("embedded ISA artifact directory is outside its artifact root")
    artifact_directory.mkdir(parents=True, exist_ok=True)
    return artifact_directory


def clear(architecture: Architecture, artifact_directory: Path) -> None:
    for suffix in (".assurance.json", ".elf", ".log", ".transcript.bin"):
        path = artifact_directory / f"{architecture.identifier}{suffix}"
        if path.is_file() or path.is_symlink():
            path.unlink()


def render_log(
    architecture: Architecture,
    observations: tuple[ProcessObservation, ...],
    cargo_version: str,
    rustc_version: str,
    qemu_version: str,
) -> bytes:
    lines = [
        f"architecture={architecture.identifier}",
        f"runner={architecture.runner}",
        f"cargo={cargo_version}",
        f"rustc={rustc_version}",
        f"qemu={qemu_version}",
        f"emulator-source={architecture.emulator.source_url}",
        f"emulator-source-sha256={architecture.emulator.source_sha256}",
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
