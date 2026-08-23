from __future__ import annotations

import io
import os
import sys
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest import mock

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    PeerSpec,
    PortLease,
    cargo_binary,
    environment,
    forbid_output_marker,
    reference_python,
    require_hex_output,
    require_output_marker,
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

    def test_checked_command_accepts_an_explicit_environment(self) -> None:
        output = run_checked(
            [sys.executable, "-c", "import os; print(os.environ['CASE_VALUE'])"],
            "command failed",
            command_environment={"CASE_VALUE": "configured"},
        )
        self.assertEqual(output, "configured\n")

    def test_missing_output_marker_is_structured(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            require_output_marker("other output\n", "EXPECTED", "missing result")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_MISSING)
        self.assertIn("other output", raised.exception.detail)

    def test_forbidden_output_marker_is_structured(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            forbid_output_marker("unexpected marker\n", "marker", "unexpected result")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_UNEXPECTED)
        self.assertIn("unexpected marker", raised.exception.detail)

    def test_hex_output_requires_the_expected_length(self) -> None:
        self.assertEqual(require_hex_output("a5" * 16 + "\n", 16, "missing hash"), "a5" * 16)
        with self.assertRaises(InteropFailure) as raised:
            require_hex_output("a5" * 15, 16, "missing hash")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_MISSING)

    def test_cargo_binary_uses_manifest_target_directory(self) -> None:
        metadata = '{"target_directory": "/tmp/cargo-target"}'
        with mock.patch(
            "validation.interop.harness.run_checked",
            side_effect=["", metadata],
        ) as checked:
            binary = cargo_binary(Path("crate/Cargo.toml"), "peer")
        self.assertEqual(binary, Path("/tmp/cargo-target/debug/peer"))
        self.assertEqual(checked.call_args_list[0].args[0][-3:], ["--bin", "peer", "--locked"])

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

    def test_case_waits_for_listener_and_closes_the_probe(self) -> None:
        connection = mock.Mock()
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "listener peer",
                    (sys.executable, "-c", "import time; time.sleep(30)"),
                    environment({}),
                )
            )
            with mock.patch(
                "validation.interop.harness.socket.create_connection",
                return_value=connection,
            ):
                case.wait_for_listener(peer, "127.0.0.1", 48123, 1)
        connection.close.assert_called_once_with()

    def test_case_waits_for_path_and_successful_peer_exit(self) -> None:
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "finite peer",
                    (sys.executable, "-c", "pass"),
                    environment({}),
                )
            )
            ready = case.work / "ready"
            ready.touch()
            case.wait_for_path(peer, ready, 1)
            case.wait_for_exit(peer, 1)

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
