from __future__ import annotations

import os
from pathlib import Path

from validation.hardening.embedded_isa.contract import (
    ROOT,
    Architecture,
    HostedPackages,
    SourceArchive,
)
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
    host_cargo_version: str,
    host_rustc_version: str,
    target_cargo_version: str,
    target_rustc_version: str,
    linker_identity: str,
    qemu_version: str,
) -> bytes:
    lines = [
        f"architecture={architecture.identifier}",
        f"runner={architecture.runner}",
        f"host-cargo={host_cargo_version}",
        f"host-rustc={host_rustc_version}",
        f"target-cargo={target_cargo_version}",
        f"target-rustc={target_rustc_version}",
        f"target-linker={linker_identity}",
        f"qemu={qemu_version}",
    ]
    match architecture.emulator.acquisition:
        case SourceArchive(source_url=url, source_sha256=checksum):
            lines.extend(
                (f"emulator-source={url}", f"emulator-source-sha256={checksum}")
            )
        case HostedPackages(packages=packages):
            for package in packages:
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
