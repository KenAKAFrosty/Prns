from __future__ import annotations

import io
import os
import sys
import unittest
from contextlib import redirect_stderr
from unittest import mock

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    PeerSpec,
    PortLease,
    environment,
    reference_python,
    run_checked,
)


class InteropHarnessTests(unittest.TestCase):
    def test_reference_python_requires_runner_configuration(self) -> None:
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(InteropFailure) as raised:
                reference_python()
        self.assertEqual(raised.exception.kind, FailureKind.MISSING_REFERENCE_INTERPRETER)

    def test_checked_command_preserves_output_on_failure(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            run_checked(
                [sys.executable, "-c", "print('command-evidence'); raise SystemExit(7)"],
                "command failed",
            )
        self.assertEqual(raised.exception.kind, FailureKind.COMMAND_FAILED)
        self.assertIn("command-evidence", raised.exception.detail)

    def test_case_waits_for_marker_and_stops_peer(self) -> None:
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "marker/peer",
                    (
                        sys.executable,
                        "-c",
                        "import time; print('READY', flush=True); time.sleep(30)",
                    ),
                    environment({}),
                )
            )
            self.assertEqual(peer.log_path.name, "00-marker-peer.log")
            case.wait_for(peer, "READY", 2)
        self.assertIsNotNone(peer.process.poll())

    def test_early_peer_exit_is_structured(self) -> None:
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "short peer",
                    (sys.executable, "-c", "raise SystemExit(9)"),
                    environment({}),
                )
            )
            with self.assertRaises(InteropFailure) as raised:
                case.wait_for(peer, "NEVER", 2)
        self.assertEqual(raised.exception.kind, FailureKind.PEER_EXITED)

    def test_marker_timeout_is_structured(self) -> None:
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "quiet peer",
                    (sys.executable, "-c", "import time; time.sleep(30)"),
                    environment({}),
                )
            )
            with self.assertRaises(InteropFailure) as raised:
                case.wait_for(peer, "NEVER", 0.1)
        self.assertEqual(raised.exception.kind, FailureKind.MARKER_TIMEOUT)

    def test_failure_prints_peer_logs(self) -> None:
        stderr = io.StringIO()
        with self.assertRaises(InteropFailure), redirect_stderr(stderr):
            with InteropCase() as case:
                peer = case.start(
                    PeerSpec(
                        "evidence peer",
                        (
                            sys.executable,
                            "-c",
                            "import time; print('evidence', flush=True); time.sleep(30)",
                        ),
                        environment({}),
                    )
                )
                case.wait_for(peer, "evidence", 2)
                raise InteropFailure(FailureKind.COMMAND_FAILED, "forced")
        self.assertIn("evidence peer log:", stderr.getvalue())
        self.assertIn("evidence", stderr.getvalue())

    def test_port_lease_holds_and_releases_the_port(self) -> None:
        listener = mock.Mock()
        listener.getsockname.return_value = ("127.0.0.1", 48123)
        with mock.patch("validation.interop.harness.socket.socket", return_value=listener):
            lease = PortLease()
        self.assertEqual(lease.port, 48123)
        listener.bind.assert_called_once_with(("127.0.0.1", 0))
        lease.release()
        lease.release()
        listener.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
