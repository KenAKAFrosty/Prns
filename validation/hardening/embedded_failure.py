from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from validation.hardening.embedded_execution import ExitReason, ProcessObservation


class FailureKind(Enum):
    CRASH = "crash"
    SCENARIO_MISMATCH = "scenario-mismatch"
    STRUCTURAL_VIOLATION = "structural-violation"
    TIMEOUT = "timeout"
    TOOL_FAILURE = "tool-failure"


@dataclass(frozen=True)
class ProofFailure:
    kind: FailureKind
    diagnostic: str


class ProofExecutionError(RuntimeError):
    def __init__(self, failure: ProofFailure):
        self.failure = failure
        super().__init__(failure.diagnostic)


def execution_error(kind: FailureKind, diagnostic: str) -> ProofExecutionError:
    return ProofExecutionError(ProofFailure(kind=kind, diagnostic=diagnostic))


def require_process_success(
    observation: ProcessObservation,
    name: str,
    nonzero_kind: FailureKind,
) -> None:
    if observation.reason is ExitReason.TIMED_OUT:
        raise execution_error(FailureKind.TIMEOUT, f"{name} timed out")
    if observation.returncode != 0:
        raise execution_error(
            nonzero_kind,
            f"{name} exited with status {observation.returncode}",
        )
