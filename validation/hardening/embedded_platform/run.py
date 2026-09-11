from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

from validation.hardening import embedded_execution
from validation.hardening.embedded_execution import ExitReason, ProcessObservation
from validation.hardening.embedded_isa.architecture import ArchitectureAdapterError
from validation.hardening.embedded_isa.contract import (
    InventoryError as IsaInventoryError,
    load_inventory as load_isa_inventory,
)
from validation.hardening.embedded_isa.emulator import (
    executable as architecture_emulator_executable,
    require_identity as require_architecture_emulator_identity,
)
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.toolchain import resolve as resolve_target_toolchain
from validation.hardening.embedded_platform import (
    artifacts,
    build as platform_build,
    emulator as platform_emulator,
    milestone,
    proof,
)
from validation.hardening.embedded_platform.contract import (
    ROOT,
    InventoryError,
    RenodeExecution,
    load_inventory,
)
from validation.hardening.embedded_platform.discovery import file_sha256
from validation.hardening.embedded_platform.error import EmbeddedPlatformError
from validation.hardening.embedded_platform.platform import (
    nrf52840,
    qemu_command,
    renode_command,
)


DOCTOR = "./tools/prns doctor embedded-assurance"


def execute(
    command: tuple[str, ...],
    timeout_seconds: int,
    environment: dict[str, str] | None = None,
    cwd: Path = ROOT,
) -> ProcessObservation:
    print(f"[embedded-platform] {' '.join(command)}", flush=True)
    observation = embedded_execution.execute(command, cwd, timeout_seconds, environment)
    sys.stdout.buffer.write(observation.output())
    sys.stdout.flush()
    return observation


def require_success(observation: ProcessObservation, name: str) -> None:
    if observation.reason is ExitReason.TIMED_OUT:
        raise EmbeddedPlatformError(f"{name} timed out")
    if observation.returncode != 0:
        raise EmbeddedPlatformError(f"{name} exited with status {observation.returncode}")


def tool_version(command: tuple[str, ...], expected_prefix: str) -> str:
    version, _ = observed_tool_version(command, expected_prefix)
    return version


def observed_tool_version(
    command: tuple[str, ...], expected_prefix: str
) -> tuple[str, ProcessObservation]:
    observation = execute(command, 30)
    try:
        require_success(observation, command[0])
    except EmbeddedPlatformError as error:
        raise EmbeddedPlatformError(f"{error}; run {DOCTOR}") from error
    output = observation.output().decode("utf-8", errors="replace").strip()
    if not output:
        raise EmbeddedPlatformError(f"{command[0]} returned an empty identity")
    version = output.splitlines()[0].strip()
    if not version.startswith(expected_prefix):
        raise EmbeddedPlatformError(
            f"unexpected {command[0]} identity {version!r}; "
            f"expected {expected_prefix!r}; run {DOCTOR}"
        )
    return version, observation


def renode_identity(
    execution: RenodeExecution, executable: Path
) -> tuple[str, ProcessObservation]:
    observation = execute((str(executable), "--version"), 30)
    require_success(observation, execution.emulator.executable)
    actual = tuple(
        line.strip()
        for line in observation.output().decode("utf-8", errors="replace").splitlines()
        if line.strip()
    )
    if actual != execution.emulator.identity:
        raise EmbeddedPlatformError(
            f"emulator identity is {actual!r}, expected {execution.emulator.identity!r}; "
            f"run {DOCTOR}"
        )
    return "; ".join(actual), observation


def run(suite: str) -> None:
    inventory = load_inventory()
    isa_inventory = load_isa_inventory()
    if inventory.rust_toolchain != isa_inventory.rust_toolchain:
        raise EmbeddedPlatformError(
            "embedded platform and ISA inventories require different Rust toolchains"
        )
    platform = inventory.platform_for_suite(suite)
    architecture = isa_inventory.architecture_named(platform.architecture)
    artifact_directory = artifacts.directory()
    artifacts.clear(platform, artifact_directory)
    host_cargo_version = tool_version(
        ("cargo", f"+{inventory.rust_toolchain}", "--version"), "cargo "
    )
    host_rustc_version = tool_version(
        ("rustc", f"+{inventory.rust_toolchain}", "--version"), "rustc "
    )
    target_toolchain = resolve_target_toolchain(
        architecture,
        inventory.rust_toolchain,
        host_cargo_version,
        host_rustc_version,
    )
    observations = []

    execution = platform.execution
    if isinstance(execution, RenodeExecution):
        emulator = platform_emulator.renode_executable(execution)
        identity, identity_observation = renode_identity(execution, emulator)
        observations.append(identity_observation)
        description, description_sha256 = platform_emulator.platform_description(
            execution, emulator
        )
        effective_description = artifact_directory / f"{platform.identifier}.repl"
        nrf52840.materialize_offline_model(description, effective_description)
        effective_description_sha256 = file_sha256(effective_description)
        emulator_evidence = platform_emulator.renode_evidence(
            execution,
            emulator,
            identity,
            description,
            description_sha256,
            effective_description,
            effective_description_sha256,
        )
    else:
        emulator = architecture_emulator_executable(
            architecture, target_toolchain.search_paths
        )
        identity, identity_observation = observed_tool_version(
            (str(emulator), "--version"), "QEMU emulator version "
        )
        require_architecture_emulator_identity(architecture, identity)
        observations.append(identity_observation)
        emulator_evidence = platform_emulator.qemu_evidence(
            architecture, emulator, identity
        )

    target = artifact_directory / f"{platform.identifier}.elf"
    transcript_path = artifact_directory / f"{platform.identifier}.transcript"
    environment = platform_build.environment(platform, target_toolchain)
    try:
        build = execute(
            platform_build.cargo_command(platform, target_toolchain.channel),
            900,
            environment,
            platform.build.workspace,
        )
        observations.append(build)
        require_success(build, f"{platform.identifier} integration build")
        shutil.copyfile(platform_build.executable(platform), target)

        if isinstance(execution, RenodeExecution):
            script = artifact_directory / f"{platform.identifier}.resc"
            config = artifact_directory / f"{platform.identifier}.config"
            emulation = execute(
                renode_command(
                    platform,
                    emulator,
                    target,
                    effective_description,
                    script,
                    config,
                ),
                platform.timeout_seconds,
            )
        else:
            flash_image = artifact_directory / f"{platform.identifier}.flash.bin"
            packaging = execute(
                platform_build.image_command(
                    platform,
                    inventory.rust_toolchain,
                    target,
                    flash_image,
                ),
                900,
                environment,
            )
            observations.append(packaging)
            require_success(packaging, f"{platform.identifier} flash image packaging")
            emulation = execute(
                qemu_command(platform, emulator, flash_image),
                platform.timeout_seconds,
                environment,
            )
        observations.append(emulation)
        require_success(emulation, f"{platform.identifier} emulation")
        transcript_path.write_bytes(milestone.parse(emulation.output(), platform))
    finally:
        log = artifact_directory / f"{platform.identifier}.log"
        log.write_bytes(
            artifacts.render_log(
                platform,
                tuple(observations),
                target_toolchain.cargo_version,
                target_toolchain.rustc_version,
                target_toolchain.linker_identity,
                emulator_evidence,
            )
        )
    proof.record(
        platform,
        emulator,
        target,
        transcript_path,
        log,
        target_toolchain.cargo_version,
        target_toolchain.rustc_version,
        target_toolchain.linker_identity,
        identity,
        platform.emulator_kind,
        target_toolchain.proof_sources,
    )
    print(
        f"EMBEDDED_PLATFORM_OK platform={platform.identifier} "
        f"profile={platform.memory_profile} milestone={platform.milestone.value}"
    )


def main() -> int:
    suite = os.environ.get("PRNS_VALIDATION_SUITE")
    if suite is None:
        print("EMBEDDED_PLATFORM_ERROR: PRNS_VALIDATION_SUITE is missing", file=sys.stderr)
        return 1
    try:
        run(suite)
        return 0
    except (
        ArchitectureAdapterError,
        EmbeddedIsaError,
        EmbeddedPlatformError,
        IsaInventoryError,
        InventoryError,
        OSError,
        subprocess.SubprocessError,
    ) as error:
        print(f"EMBEDDED_PLATFORM_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
