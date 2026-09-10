from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

from validation.hardening.embedded_isa import artifacts, proof, transcript
from validation.hardening.embedded_isa.architecture import (
    ArchitectureAdapterError,
    command_for,
)
from validation.hardening.embedded_isa.contract import (
    ROOT,
    Architecture,
    InventoryError,
    Kernel,
    load_inventory,
)
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.process import execute, require_success, tool_version


def require_emulator_identity(architecture: Architecture, actual: str) -> None:
    expected = f"QEMU emulator version {architecture.emulator.version}"
    if actual != expected:
        raise EmbeddedIsaError(
            f"emulator identity is {actual!r}, expected {expected!r}"
        )


def emulator_executable(architecture: Architecture) -> Path:
    discovered = shutil.which(architecture.emulator.executable)
    if discovered is None:
        raise EmbeddedIsaError(
            f"required emulator {architecture.emulator.executable} is unavailable"
        )
    executable = Path(discovered).resolve(strict=True)
    if not executable.is_file():
        raise EmbeddedIsaError(f"emulator is not a regular file: {executable}")
    return executable


def target_directory() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if configured is None:
        return ROOT / "target"
    path = Path(configured)
    return path if path.is_absolute() else ROOT / path


def target_executable(architecture: Architecture) -> Path:
    executable = (
        target_directory()
        / architecture.rust_target
        / "release"
        / architecture.binary
    ).resolve(strict=True)
    if not executable.is_file():
        raise EmbeddedIsaError(f"target executable is not a regular file: {executable}")
    return executable


def cargo_command(
    kernel: Kernel,
    toolchain: str,
    operation: str,
    feature: str,
    binary: str,
) -> tuple[str, ...]:
    return (
        "cargo",
        f"+{toolchain}",
        operation,
        "--release",
        "--locked",
        "--manifest-path",
        str(kernel.manifest),
        "--package",
        kernel.package,
        "--features",
        feature,
        "--bin",
        binary,
    )


def run(suite: str) -> None:
    inventory = load_inventory()
    architecture = inventory.architecture_for_suite(suite)
    artifact_directory = artifacts.directory()
    artifacts.clear(architecture, artifact_directory)
    emulator = emulator_executable(architecture)
    cargo_version = tool_version(
        ("cargo", f"+{inventory.rust_toolchain}", "--version"), "cargo "
    )
    rustc_version = tool_version(
        ("rustc", f"+{inventory.rust_toolchain}", "--version"), "rustc "
    )
    qemu_version = tool_version((str(emulator), "--version"), "QEMU emulator version ")
    require_emulator_identity(architecture, qemu_version)

    observations = []
    host = execute(
        cargo_command(
            inventory.kernel,
            inventory.rust_toolchain,
            "run",
            inventory.kernel.host_feature,
            inventory.kernel.host_binary,
        ),
        900,
    )
    observations.append(host)
    try:
        require_success(host, "host reference")
        host_transcript = transcript.parse(
            host.output(), inventory.kernel.completed_scenarios
        )
        build = execute(
            cargo_command(
                inventory.kernel,
                inventory.rust_toolchain,
                "build",
                architecture.feature,
                architecture.binary,
            )
            + ("--target", architecture.rust_target),
            900,
        )
        observations.append(build)
        require_success(build, f"{architecture.identifier} build")
        built_target = target_executable(architecture)
        target = artifact_directory / f"{architecture.identifier}.elf"
        shutil.copyfile(built_target, target)
        emulation = execute(
            command_for(architecture.identifier, emulator, target),
            architecture.timeout_seconds,
        )
        observations.append(emulation)
        require_success(emulation, f"{architecture.identifier} emulation")
        target_transcript = transcript.parse(
            emulation.output(), inventory.kernel.completed_scenarios
        )
        transcript.require_match(host_transcript, target_transcript)
    finally:
        log = artifact_directory / f"{architecture.identifier}.log"
        log.write_bytes(
            artifacts.render_log(
                architecture,
                tuple(observations),
                cargo_version,
                rustc_version,
                qemu_version,
            )
        )

    transcript_path = artifact_directory / f"{architecture.identifier}.transcript.bin"
    transcript_path.write_bytes(target_transcript.events)
    proof.record(
        architecture,
        inventory.kernel,
        emulator,
        target,
        transcript_path,
        log,
        cargo_version,
        rustc_version,
        qemu_version,
    )
    print(
        f"EMBEDDED_ISA_OK architecture={architecture.identifier} "
        f"scenarios={target_transcript.completed_scenarios} "
        f"bytes={len(target_transcript.events)} digest={target_transcript.digest}"
    )


def main() -> int:
    suite = os.environ.get("PRNS_VALIDATION_SUITE")
    if suite is None:
        print("EMBEDDED_ISA_ERROR: PRNS_VALIDATION_SUITE is missing", file=sys.stderr)
        return 1
    try:
        run(suite)
        return 0
    except (
        ArchitectureAdapterError,
        EmbeddedIsaError,
        InventoryError,
        OSError,
        subprocess.SubprocessError,
    ) as error:
        print(f"EMBEDDED_ISA_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
