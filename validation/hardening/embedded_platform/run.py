from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

from validation.hardening import embedded_execution
from validation.hardening.embedded_execution import ExitReason, ProcessObservation
from validation.hardening.embedded_platform import artifacts, proof
from validation.hardening.embedded_platform.contract import (
    ROOT,
    EmulatorKind,
    InventoryError,
    Platform,
    load_inventory,
)
from validation.hardening.embedded_platform.discovery import (
    RENODE_EXECUTABLE_ENV,
    RENODE_ROOT_ENV,
    description_candidates,
    file_sha256,
)
from validation.hardening.embedded_platform.error import EmbeddedPlatformError
from validation.hardening.embedded_platform.platform import command_for, nrf52840


DOCTOR = "./tools/prns doctor embedded-assurance"
MEMORY_PROFILE_ENV = "PRNS_ASSURANCE_MEMORY_PROFILE"


def execute(
    command: tuple[str, ...],
    timeout_seconds: int,
    environment: dict[str, str] | None = None,
) -> ProcessObservation:
    print(f"[embedded-platform] {' '.join(command)}", flush=True)
    observation = embedded_execution.execute(
        command, ROOT, timeout_seconds, environment
    )
    sys.stdout.buffer.write(observation.output())
    sys.stdout.flush()
    return observation


def require_success(observation: ProcessObservation, name: str) -> None:
    if observation.reason is ExitReason.TIMED_OUT:
        raise EmbeddedPlatformError(f"{name} timed out")
    if observation.returncode != 0:
        raise EmbeddedPlatformError(
            f"{name} exited with status {observation.returncode}"
        )


def tool_version(command: tuple[str, ...], expected_prefix: str) -> str:
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
    return version


def emulator_executable(platform: Platform) -> Path:
    override = os.environ.get(RENODE_EXECUTABLE_ENV)
    discovered = override or shutil.which(platform.emulator.executable)
    if discovered is None:
        raise EmbeddedPlatformError(
            f"required emulator {platform.emulator.executable} is unavailable; run {DOCTOR}"
        )
    executable = Path(discovered).resolve(strict=True)
    if not executable.is_file():
        raise EmbeddedPlatformError(f"emulator is not a regular file: {executable}")
    return executable


def emulator_identity(
    platform: Platform, executable: Path
) -> tuple[str, ProcessObservation]:
    observation = execute((str(executable), "--version"), 30)
    require_success(observation, platform.emulator.executable)
    actual = tuple(
        line.strip()
        for line in observation.output().decode("utf-8", errors="replace").splitlines()
        if line.strip()
    )
    if actual != platform.emulator.identity:
        raise EmbeddedPlatformError(
            f"emulator identity is {actual!r}, expected {platform.emulator.identity!r}; "
            f"run {DOCTOR}"
        )
    return "; ".join(actual), observation


def platform_description(platform: Platform, executable: Path) -> tuple[Path, str]:
    relative = Path(platform.platform_description)
    inspected = []
    roots = description_candidates(executable, os.environ.get(RENODE_ROOT_ENV))
    for root in roots:
        candidate = root / relative
        try:
            resolved = candidate.resolve(strict=True)
        except OSError:
            continue
        if not resolved.is_file():
            continue
        actual = file_sha256(resolved)
        inspected.append((resolved, actual))
        if actual == platform.platform_description_sha256:
            return resolved, actual
    if inspected:
        found = ", ".join(f"{path}={digest}" for path, digest in inspected)
        raise EmbeddedPlatformError(
            f"platform model checksum mismatch; expected "
            f"{platform.platform_description_sha256}, found {found}; run {DOCTOR}"
        )
    raise EmbeddedPlatformError(
        f"cannot locate {platform.platform_description} beside {executable}; "
        f"set {RENODE_ROOT_ENV} or run {DOCTOR}"
    )


def target_directory() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if configured is None:
        return ROOT / "target"
    path = Path(configured)
    return path if path.is_absolute() else ROOT / path


def target_executable(platform: Platform) -> Path:
    executable = (
        target_directory() / platform.rust_target / "release" / platform.binary
    ).resolve(strict=True)
    if not executable.is_file():
        raise EmbeddedPlatformError(
            f"platform executable is not a regular file: {executable}"
        )
    return executable


def cargo_command(platform: Platform, toolchain: str) -> tuple[str, ...]:
    return (
        "cargo",
        f"+{toolchain}",
        "build",
        "--release",
        "--locked",
        "--manifest-path",
        str(ROOT / "personal-hopspot" / "assurance-kernel" / "Cargo.toml"),
        "--package",
        "personal-hopspot-assurance-kernel",
        "--no-default-features",
        "--features",
        platform.feature,
        "--bin",
        platform.binary,
        "--target",
        platform.rust_target,
    )


def run(suite: str) -> None:
    inventory = load_inventory()
    platform = inventory.platform_for_suite(suite)
    if platform.emulator.kind is not EmulatorKind.RENODE:
        raise EmbeddedPlatformError(
            f"platform runner does not support {platform.emulator.kind.value!r} yet"
        )
    artifact_directory = artifacts.directory()
    artifacts.clear(platform, artifact_directory)
    cargo_version = tool_version(
        ("cargo", f"+{inventory.rust_toolchain}", "--version"), "cargo "
    )
    rustc_version = tool_version(
        ("rustc", f"+{inventory.rust_toolchain}", "--version"), "rustc "
    )
    linker_identity = f"rust-lld bundled with {rustc_version}"
    emulator = emulator_executable(platform)
    identity, identity_observation = emulator_identity(platform, emulator)
    description, description_sha256 = platform_description(platform, emulator)
    effective_description = artifact_directory / f"{platform.identifier}.repl"
    nrf52840.materialize_offline_model(description, effective_description)
    effective_description_sha256 = file_sha256(effective_description)
    observations = [identity_observation]
    executable_sha256 = file_sha256(emulator)
    try:
        environment = dict(os.environ)
        environment[MEMORY_PROFILE_ENV] = platform.memory_profile
        build = execute(
            cargo_command(platform, inventory.rust_toolchain), 900, environment
        )
        observations.append(build)
        require_success(build, f"{platform.identifier} integration build")
        built = target_executable(platform)
        target = artifact_directory / f"{platform.identifier}.elf"
        shutil.copyfile(built, target)
        script = artifact_directory / f"{platform.identifier}.resc"
        config = artifact_directory / f"{platform.identifier}.config"
        emulation = execute(
            command_for(
                platform,
                emulator,
                target,
                effective_description,
                script,
                config,
            ),
            platform.timeout_seconds,
        )
        observations.append(emulation)
        require_success(emulation, f"{platform.identifier} emulation")
        transcript = nrf52840.parse_milestone(emulation.output(), platform)
        transcript_path = artifact_directory / f"{platform.identifier}.transcript"
        transcript_path.write_bytes(transcript)
    finally:
        log = artifact_directory / f"{platform.identifier}.log"
        log.write_bytes(
            artifacts.render_log(
                platform,
                tuple(observations),
                cargo_version,
                rustc_version,
                linker_identity,
                identity,
                executable_sha256,
                description,
                description_sha256,
                effective_description,
                effective_description_sha256,
            )
        )
    proof.record(
        platform,
        emulator,
        target,
        transcript_path,
        log,
        cargo_version,
        rustc_version,
        linker_identity,
        identity,
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
        EmbeddedPlatformError,
        InventoryError,
        OSError,
        subprocess.SubprocessError,
    ) as error:
        print(f"EMBEDDED_PLATFORM_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
