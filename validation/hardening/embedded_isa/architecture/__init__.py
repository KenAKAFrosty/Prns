from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from validation.hardening.embedded_isa.architecture import (
    riscv32imac,
    thumbv7em,
    xtensa_esp32s3,
)


class ArchitectureAdapterError(RuntimeError):
    pass


Adapter = Callable[[Path, Path], tuple[str, ...]]


@dataclass(frozen=True)
class TargetBuildAdapter:
    linker: str
    cargo_arguments: Callable[[Path, str], tuple[str, ...]]

ADAPTERS: dict[str, Adapter] = {
    "riscv32imac": riscv32imac.command,
    "thumbv7em": thumbv7em.command,
    "xtensa-esp32s3": xtensa_esp32s3.command,
}

TARGET_BUILD_ADAPTERS = {
    "xtensa-esp32s3": TargetBuildAdapter(
        linker="xtensa-esp32s3-elf-gcc",
        cargo_arguments=xtensa_esp32s3.cargo_arguments,
    ),
}


def command_for(architecture: str, emulator: Path, executable: Path) -> tuple[str, ...]:
    try:
        adapter = ADAPTERS[architecture]
    except KeyError as error:
        raise ArchitectureAdapterError(
            f"no target-ISA adapter for architecture {architecture}"
        ) from error
    return adapter(emulator, executable)


def target_build_for(architecture: str) -> TargetBuildAdapter:
    try:
        return TARGET_BUILD_ADAPTERS[architecture]
    except KeyError as error:
        raise ArchitectureAdapterError(
            f"no specialized target-ISA build adapter for architecture {architecture}"
        ) from error
