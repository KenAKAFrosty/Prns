from __future__ import annotations

import os
import shutil
from pathlib import Path

from validation.hardening.embedded_isa.contract import Architecture
from validation.hardening.embedded_isa.error import EmbeddedIsaError


DOCTOR = "./tools/prns doctor embedded-assurance"


def require_identity(architecture: Architecture, actual: str) -> None:
    expected = architecture.emulator.identity.banner
    if actual != expected:
        raise EmbeddedIsaError(
            f"emulator identity is {actual!r}, expected {expected!r}; run {DOCTOR}"
        )


def executable(
    architecture: Architecture, search_paths: tuple[Path, ...] = ()
) -> Path:
    path = os.pathsep.join(str(entry) for entry in search_paths) or None
    discovered = shutil.which(architecture.emulator.executable, path=path)
    if discovered is None and path is not None:
        discovered = shutil.which(architecture.emulator.executable)
    if discovered is None:
        raise EmbeddedIsaError(
            f"required emulator {architecture.emulator.executable} is unavailable; "
            f"run {DOCTOR}"
        )
    resolved = Path(discovered).resolve(strict=True)
    if not resolved.is_file():
        raise EmbeddedIsaError(f"emulator is not a regular file: {resolved}")
    return resolved
