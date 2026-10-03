from __future__ import annotations

import subprocess
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Mapping


class ExitReason(Enum):
    EXITED = "exited"
    TIMED_OUT = "timed-out"


@dataclass(frozen=True)
class ProcessObservation:
    command: tuple[str, ...]
    reason: ExitReason
    returncode: int | None
    stdout: bytes
    stderr: bytes

    def output(self) -> bytes:
        return self.stdout + self.stderr


def execute(
    command: tuple[str, ...],
    cwd: Path,
    timeout_seconds: int,
    environment: Mapping[str, str] | None = None,
) -> ProcessObservation:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=environment,
            capture_output=True,
            timeout=timeout_seconds,
        )
        return ProcessObservation(
            command=command,
            reason=ExitReason.EXITED,
            returncode=result.returncode,
            stdout=result.stdout,
            stderr=result.stderr,
        )
    except subprocess.TimeoutExpired as error:
        return ProcessObservation(
            command=command,
            reason=ExitReason.TIMED_OUT,
            returncode=None,
            stdout=error.stdout or b"",
            stderr=error.stderr or b"",
        )
