from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from validation.hardening.embedded_isa.architecture import riscv32imac, thumbv7em


class ArchitectureAdapterError(RuntimeError):
    pass


Adapter = Callable[[Path, Path], tuple[str, ...]]

ADAPTERS: dict[str, Adapter] = {
    "riscv32imac": riscv32imac.command,
    "thumbv7em": thumbv7em.command,
}


def command_for(architecture: str, emulator: Path, executable: Path) -> tuple[str, ...]:
    try:
        adapter = ADAPTERS[architecture]
    except KeyError as error:
        raise ArchitectureAdapterError(
            f"no target-ISA adapter for architecture {architecture}"
        ) from error
    return adapter(emulator, executable)
