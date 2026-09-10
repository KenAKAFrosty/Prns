from __future__ import annotations

import subprocess
import sys
from dataclasses import dataclass
from enum import Enum

from validation.hardening.embedded_isa.contract import ROOT
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.transcript import concise


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


def execute(command: tuple[str, ...], timeout_seconds: int) -> ProcessObservation:
    print(f"[embedded-isa] {' '.join(command)}", flush=True)
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            timeout=timeout_seconds,
        )
        observation = ProcessObservation(
            command=command,
            reason=ExitReason.EXITED,
            returncode=result.returncode,
            stdout=result.stdout,
            stderr=result.stderr,
        )
    except subprocess.TimeoutExpired as error:
        observation = ProcessObservation(
            command=command,
            reason=ExitReason.TIMED_OUT,
            returncode=None,
            stdout=error.stdout or b"",
            stderr=error.stderr or b"",
        )
    sys.stdout.buffer.write(concise(observation.output()))
    sys.stdout.flush()
    return observation


def require_success(observation: ProcessObservation, name: str) -> None:
    if observation.reason is ExitReason.TIMED_OUT:
        raise EmbeddedIsaError(f"{name} timed out")
    if observation.returncode != 0:
        raise EmbeddedIsaError(f"{name} exited with status {observation.returncode}")


def tool_version(command: tuple[str, ...], expected_prefix: str) -> str:
    observation = execute(command, 30)
    require_success(observation, command[0])
    output = observation.output().decode("utf-8", errors="replace").strip()
    if not output:
        raise EmbeddedIsaError(f"{command[0]} returned an empty identity")
    version = output.splitlines()[0]
    if not version.startswith(expected_prefix):
        raise EmbeddedIsaError(
            f"unexpected {command[0]} identity {version!r}; expected {expected_prefix!r}"
        )
    return version
