from __future__ import annotations

import unittest

from validation.hardening.embedded_execution import ExitReason, ProcessObservation
from validation.hardening.embedded_failure import (
    FailureKind,
    ProofExecutionError,
    require_process_success,
)


class EmbeddedFailureTests(unittest.TestCase):
    def test_timeout_has_one_classification_for_every_runner(self) -> None:
        observation = ProcessObservation(
            command=("runner",),
            reason=ExitReason.TIMED_OUT,
            returncode=None,
            stdout=b"partial output",
            stderr=b"",
        )

        with self.assertRaises(ProofExecutionError) as raised:
            require_process_success(observation, "target execution", FailureKind.CRASH)

        self.assertEqual(raised.exception.failure.kind, FailureKind.TIMEOUT)
        self.assertEqual(raised.exception.failure.diagnostic, "target execution timed out")

    def test_nonzero_exit_preserves_the_runner_owned_classification(self) -> None:
        observation = ProcessObservation(
            command=("runner",),
            reason=ExitReason.EXITED,
            returncode=17,
            stdout=b"",
            stderr=b"failure",
        )

        for kind in (
            FailureKind.CRASH,
            FailureKind.SCENARIO_MISMATCH,
            FailureKind.STRUCTURAL_VIOLATION,
            FailureKind.TOOL_FAILURE,
        ):
            with self.subTest(kind=kind):
                with self.assertRaises(ProofExecutionError) as raised:
                    require_process_success(observation, "target execution", kind)
                self.assertEqual(raised.exception.failure.kind, kind)
                self.assertEqual(
                    raised.exception.failure.diagnostic,
                    "target execution exited with status 17",
                )


if __name__ == "__main__":
    unittest.main()
