from __future__ import annotations

import os
from pathlib import Path

from validation.hardening.embedded_isa.toolchain import TargetToolchain
from validation.hardening.embedded_platform.contract import (
    ROOT,
    ArchitectureQemuExecution,
    Platform,
)
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


MEMORY_PROFILE_ENV = "PRNS_ASSURANCE_MEMORY_PROFILE"


def environment(
    platform: Platform, toolchain: TargetToolchain
) -> dict[str, str]:
    values = dict(os.environ)
    values.update(toolchain.environment)
    values[MEMORY_PROFILE_ENV] = platform.memory_profile
    return values


def executable(platform: Platform) -> Path:
    target = (
        target_directory(platform)
        / platform.rust_target
        / "release"
        / platform.build.binary
    ).resolve(strict=True)
    if not target.is_file():
        raise EmbeddedPlatformError(f"platform executable is not a regular file: {target}")
    return target


def cargo_command(platform: Platform, toolchain: str) -> tuple[str, ...]:
    command = [
        "cargo",
        f"+{toolchain}",
        "build",
        "--release",
        "--locked",
        "--manifest-path",
        str(platform.build.manifest),
        "--package",
        platform.build.package,
        "--no-default-features",
    ]
    if platform.build.features:
        command.extend(("--features", ",".join(platform.build.features)))
    command.extend(("--bin", platform.build.binary, "--target", platform.rust_target))
    if toolchain == "esp":
        command.append("-Zbuild-std=core,alloc")
    return tuple(command)


def image_command(
    platform: Platform, toolchain: str, elf: Path, output: Path
) -> tuple[str, ...]:
    execution = platform.execution
    if not isinstance(execution, ArchitectureQemuExecution):
        raise EmbeddedPlatformError("only QEMU platform pilots require flash images")
    return (
        "cargo",
        f"+{toolchain}",
        "run",
        "--locked",
        "--quiet",
        "--package",
        "personal-hopspot-builder",
        "--bin",
        "personal-hopspot-esp-image",
        "--",
        str(elf),
        execution.board,
        str(ROOT),
        str(output),
    )


def target_directory(platform: Platform) -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if configured is None:
        return platform.build.workspace / "target"
    path = Path(configured)
    return path if path.is_absolute() else platform.build.workspace / path
