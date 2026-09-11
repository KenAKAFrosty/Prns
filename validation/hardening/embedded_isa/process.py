from __future__ import annotations

import sys

from validation.hardening.embedded_execution import (
    ExitReason,
    ProcessObservation,
    execute as execute_process,
)
from validation.hardening.embedded_isa.contract import ROOT
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.transcript import concise


def execute(command: tuple[str, ...], timeout_seconds: int) -> ProcessObservation:
    print(f"[embedded-isa] {' '.join(command)}", flush=True)
    observation = execute_process(command, ROOT, timeout_seconds)
    sys.stdout.buffer.write(concise(observation.output()))
    sys.stdout.flush()
    return observation


def require_success(observation: ProcessObservation, name: str) -> None:
    if observation.reason is ExitReason.TIMED_OUT:
        raise EmbeddedIsaError(f"{name} timed out")
    if observation.returncode != 0:
        raise EmbeddedIsaError(f"{name} exited with status {observation.returncode}")


def tool_version(
    command: tuple[str, ...], expected_prefix: str, doctor: str | None = None
) -> str:
    observation = execute(command, 30)
    try:
        require_success(observation, command[0])
    except EmbeddedIsaError as error:
        suffix = f"; run {doctor}" if doctor is not None else ""
        raise EmbeddedIsaError(f"{error}{suffix}") from error
    output = observation.output().decode("utf-8", errors="replace").strip()
    if not output:
        raise EmbeddedIsaError(f"{command[0]} returned an empty identity")
    version = output.splitlines()[0]
    if not version.startswith(expected_prefix):
        suffix = f"; run {doctor}" if doctor is not None else ""
        raise EmbeddedIsaError(
            f"unexpected {command[0]} identity {version!r}; "
            f"expected {expected_prefix!r}{suffix}"
        )
    return version
