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
    reference_utility,
    require_evidence,
    require_hex_output,
    require_listening_destination,
    require_output_marker,
    run_checked,
    run_checked_bytes,
    run_expect_status,
    run_expect_status_with_streams,
)


class InteropHarnessTests(unittest.TestCase):
    def test_reference_python_requires_runner_configuration(self) -> None:
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(InteropFailure) as raised:
                reference_python()
        self.assertEqual(raised.exception.kind, FailureKind.MISSING_REFERENCE_INTERPRETER)

    def test_reference_utility_is_resolved_beside_the_reference_python(self) -> None:
        with mock.patch("validation.interop.harness.reference_python") as python:
            python.return_value = Path("/oracle/bin/python")
            with mock.patch("validation.interop.harness.Path.is_file", return_value=True):
                with mock.patch("validation.interop.harness.os.access", return_value=True):
                    utility = reference_utility("rncp")
        self.assertEqual(utility, Path("/oracle/bin/rncp"))

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

    def test_expected_status_command_preserves_output(self) -> None:
        output = run_expect_status(
            [sys.executable, "-c", "print('expected-evidence'); raise SystemExit(7)"],
            7,
            "command returned the wrong status",
        )
        self.assertEqual(output, "expected-evidence\n")

    def test_expected_status_command_rejects_another_status(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            run_expect_status(
                [sys.executable, "-c", "raise SystemExit(8)"],
                7,
                "command returned the wrong status",
            )
        self.assertEqual(raised.exception.kind, FailureKind.COMMAND_FAILED)
        self.assertIn("expected status 7, got 8", raised.exception.detail)

    def test_expected_status_command_can_preserve_separate_streams(self) -> None:
        streams = run_expect_status_with_streams(
            [
                sys.executable,
                "-c",
                "import sys; print('output'); print('error', file=sys.stderr); raise SystemExit(7)",
            ],
            7,
            "command returned the wrong status",
        )
        self.assertEqual(streams.standard_output, "output\n")
        self.assertEqual(streams.standard_error, "error\n")

    def test_checked_binary_command_preserves_binary_standard_io(self) -> None:
        output = run_checked_bytes(
            [
                sys.executable,
                "-c",
                "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read()[::-1])",
            ],
            "binary command failed",
            standard_input=b"\x00\xff\x17",
        )
        self.assertEqual(output, b"\x17\xff\x00")

    def test_environment_can_remove_inherited_case_configuration(self) -> None:
        with mock.patch.dict(os.environ, {"CASE_VALUE": "inherited"}, clear=True):
            configured = environment({}, without=("CASE_VALUE",))
        self.assertNotIn("CASE_VALUE", configured)

    def test_missing_evidence_is_structured(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            require_evidence(False, "missing result")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_MISSING)

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

    def test_listening_destination_requires_the_stock_utility_shape(self) -> None:
        destination = "a5" * 16
        output = f"Listening on : <{destination}>\n"
        self.assertEqual(require_listening_destination(output, "missing listener"), destination)
        with self.assertRaises(InteropFailure) as raised:
            require_listening_destination("Listening elsewhere\n", "missing listener")
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

    def test_required_peer_exit_interrupts_an_evidence_wait(self) -> None:
        with InteropCase() as case:
            evidence = case.start(
                PeerSpec(
                    "evidence peer",
                    (sys.executable, "-c", "import time; time.sleep(30)"),
                    environment({}),
                )
            )
            required = case.start(
                PeerSpec(
                    "required peer",
                    (sys.executable, "-c", "raise SystemExit(7)"),
                    environment({}),
                )
            )
            with self.assertRaises(InteropFailure) as raised:
                case.wait_for_all(
                    [(evidence, "NEVER")],
                    2,
                    required_peers=(required,),
                )
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

    def test_case_returns_a_nonzero_peer_status(self) -> None:
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "failed peer",
                    (sys.executable, "-c", "raise SystemExit(7)"),
                    environment({}),
                )
            )
            self.assertEqual(case.wait_for_status(peer, 1), 7)

    def test_case_can_prove_a_peer_remains_running_then_terminate_it(self) -> None:
        with InteropCase() as case:
            peer = case.start(
                PeerSpec(
                    "waiting peer",
                    (sys.executable, "-c", "import time; time.sleep(30)"),
                    environment({}),
                )
            )
            case.require_running(peer, 0.1, "peer did not remain active")
            self.assertNotEqual(case.terminate(peer), 0)

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
