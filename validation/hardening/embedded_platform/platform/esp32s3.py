from pathlib import Path

from validation.hardening.embedded_platform.contract import (
    ArchitectureQemuExecution,
    Platform,
)
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


def command(platform: Platform, emulator: Path, flash_image: Path) -> tuple[str, ...]:
    execution = platform.execution
    if not isinstance(execution, ArchitectureQemuExecution):
        raise EmbeddedPlatformError("ESP32-S3 platform pilot requires QEMU")
    return (
        str(emulator),
        "-machine",
        execution.machine,
        "-cpu",
        execution.cpu,
        "-nographic",
        "-monitor",
        "none",
        "-serial",
        "none",
        "-semihosting-config",
        "enable=on,target=native",
        "-drive",
        f"file={flash_image},if=mtd,format=raw",
    )
