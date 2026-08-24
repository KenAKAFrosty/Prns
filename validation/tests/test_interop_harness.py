from __future__ import annotations

import io
import json
import os
import sys
import threading
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from types import SimpleNamespace
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
    require_no_protocol_violations_output,
    require_output_marker,
    run_checked,
    run_checked_bytes,
    run_expect_status,
    run_expect_status_with_streams,
)
from validation.interop.peers.rns_protocol_evidence import (
    PROTOCOL_EVIDENCE_FINAL,
    PROTOCOL_EVIDENCE_READY,
    PROTOCOL_EVIDENCE_SCHEMA,
    protocol_evidence_snapshot,
)


def protocol_snapshot(count: int) -> dict[str, object]:
    return {
        "schema": PROTOCOL_EVIDENCE_SCHEMA,
        "interfaces": [
            {
                "name": "test interface",
                "type": "TestInterface",
                "protocol_violations": count,
                "ifac_violations": 0,
                "packet_filter_hits": 0,
            }
        ],
    }


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
        executable = "rncp.exe" if os.name == "nt" else "rncp"
        self.assertEqual(utility, Path("/oracle/bin") / executable)

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
            command_environment={**os.environ, "CASE_VALUE": "configured"},
        )
        self.assertEqual(output, "configured\n")

    def test_checked_command_requires_utf8_output(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            run_checked(
                [
                    sys.executable,
                    "-c",
                    "import sys; sys.stdout.buffer.write(bytes([0x66, 0x8f]))",
                ],
                "command emitted invalid text",
            )
        self.assertEqual(raised.exception.kind, FailureKind.COMMAND_OUTPUT_INVALID)
        self.assertIn("combined output is not UTF-8 at byte 1", raised.exception.detail)

    def test_checked_command_decodes_multibyte_utf8_output(self) -> None:
        output = run_checked(
            [
                sys.executable,
                "-c",
                "import sys; sys.stdout.buffer.write('Φ𐑐'.encode('utf-8'))",
            ],
            "command emitted invalid text",
        )
        self.assertEqual(output, "Φ𐑐")

    def test_checked_command_configures_python_utf8_output(self) -> None:
        output = run_checked(
            [sys.executable, "-c", "import sys; print(sys.stdout.encoding)"],
            "command did not report its encoding",
            command_environment={**os.environ, "PYTHONIOENCODING": "cp1252"},
        )
        self.assertEqual(output.strip().lower(), "utf-8")

    def test_failed_command_renders_invalid_bytes_as_diagnostics(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            run_checked(
                [
                    sys.executable,
                    "-c",
                    "import sys; sys.stderr.buffer.write(bytes([0x66, 0x8f])); raise SystemExit(7)",
                ],
                "command failed",
            )
        self.assertEqual(raised.exception.kind, FailureKind.COMMAND_FAILED)
        self.assertIn("f�", raised.exception.detail)

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

    def test_expected_status_command_requires_utf8_on_each_stream(self) -> None:
        with self.assertRaises(InteropFailure) as raised:
            run_expect_status_with_streams(
                [
                    sys.executable,
                    "-c",
                    "import sys; sys.stderr.buffer.write(bytes([0x8f])); raise SystemExit(7)",
                ],
                7,
                "command emitted invalid text",
            )
        self.assertEqual(raised.exception.kind, FailureKind.COMMAND_OUTPUT_INVALID)
        self.assertIn("standard error is not UTF-8 at byte 0", raised.exception.detail)

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
        executable = "peer.exe" if os.name == "nt" else "peer"
        self.assertEqual(binary, Path("/tmp/cargo-target/debug") / executable)
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

    def test_reference_rns_protocol_evidence_is_queried_while_running(self) -> None:
        snapshot = json.dumps(protocol_snapshot(0))
        connection = mock.MagicMock()
        connection.__enter__.return_value = connection
        connection.makefile.return_value = io.BytesIO((snapshot + "\n").encode("utf-8"))
        script = f"import time; print('{PROTOCOL_EVIDENCE_READY}48123',flush=True); time.sleep(30)"
        with mock.patch(
            "validation.interop.harness.socket.create_connection",
            return_value=connection,
        ):
            with InteropCase() as case:
                peer = case.start_reference_rns(
                    PeerSpec(
                        "live reference RNS",
                        (sys.executable, "-c", script),
                        environment({}),
                    )
                )
                case.wait_for(peer, PROTOCOL_EVIDENCE_READY, 2)

    def test_reference_rns_final_protocol_evidence_supports_exited_peers(self) -> None:
        final = PROTOCOL_EVIDENCE_FINAL + json.dumps(protocol_snapshot(0))
        with InteropCase() as case:
            peer = case.start_reference_rns(
                PeerSpec(
                    "finite reference RNS",
                    (sys.executable, "-c", f"print({final!r},flush=True)"),
                    environment({}),
                )
            )
            case.wait_for_exit(peer, 2)

    def test_reference_rns_protocol_violation_fails_the_case(self) -> None:
        final = PROTOCOL_EVIDENCE_FINAL + json.dumps(protocol_snapshot(1))
        stderr = io.StringIO()
        with self.assertRaises(InteropFailure) as raised, redirect_stderr(stderr):
            with InteropCase() as case:
                peer = case.start_reference_rns(
                    PeerSpec(
                        "violating reference RNS",
                        (sys.executable, "-c", f"print({final!r},flush=True)"),
                        environment({}),
                    )
                )
                case.wait_for_exit(peer, 2)
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_UNEXPECTED)
        self.assertIn("protocol violations", raised.exception.detail)

    def test_protocol_snapshot_uses_rns_interface_counters(self) -> None:
        interface = SimpleNamespace(
            protocol_violations=2,
            ifac_violations=3,
            packet_filter_hits=4,
        )
        interface.__str__ = lambda: "ignored"
        rns = SimpleNamespace(Transport=SimpleNamespace(interfaces=[interface]))
        with mock.patch.dict(sys.modules, {"RNS": rns}):
            snapshot = protocol_evidence_snapshot()
        self.assertEqual(
            snapshot,
            {
                "schema": PROTOCOL_EVIDENCE_SCHEMA,
                "interfaces": [
                    {
                        "name": str(interface),
                        "type": "SimpleNamespace",
                        "protocol_violations": 2,
                        "ifac_violations": 3,
                        "packet_filter_hits": 4,
                    }
                ],
            },
        )

    def test_final_protocol_evidence_can_be_required_from_command_output(self) -> None:
        output = PROTOCOL_EVIDENCE_FINAL + json.dumps(protocol_snapshot(0)) + "\n"
        require_no_protocol_violations_output(output, "finite stock command")
        violating = PROTOCOL_EVIDENCE_FINAL + json.dumps(protocol_snapshot(1)) + "\n"
        with self.assertRaises(InteropFailure) as raised:
            require_no_protocol_violations_output(violating, "finite stock command")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_UNEXPECTED)

    def test_protocol_evidence_requires_the_rns_1_5_counter(self) -> None:
        snapshot = protocol_snapshot(0)
        interface = snapshot["interfaces"][0]
        interface.pop("protocol_violations")
        output = PROTOCOL_EVIDENCE_FINAL + json.dumps(snapshot) + "\n"
        with self.assertRaises(InteropFailure) as raised:
            require_no_protocol_violations_output(output, "finite stock command")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_MISSING)
        interface["protocol_violations"] = False
        output = PROTOCOL_EVIDENCE_FINAL + json.dumps(snapshot) + "\n"
        with self.assertRaises(InteropFailure) as raised:
            require_no_protocol_violations_output(output, "finite stock command")
        self.assertEqual(raised.exception.kind, FailureKind.EVIDENCE_MISSING)

    def test_non_protocol_counters_remain_informational(self) -> None:
        snapshot = protocol_snapshot(0)
        interface = snapshot["interfaces"][0]
        interface["ifac_violations"] = 2
        interface["packet_filter_hits"] = 3
        output = PROTOCOL_EVIDENCE_FINAL + json.dumps(snapshot) + "\n"
        require_no_protocol_violations_output(output, "finite stock command")

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

    def test_workspace_cleanup_retries_a_transient_error(self) -> None:
        case = InteropCase()
        case._temporary.cleanup()
        temporary = mock.Mock()
        temporary.cleanup.side_effect = [OSError("busy"), None]
        case._temporary = temporary
        with mock.patch("validation.interop.harness.time.sleep") as sleep:
            case._cleanup_workspace()
        self.assertEqual(temporary.cleanup.call_count, 2)
        sleep.assert_called_once_with(0.05)

    def test_workspace_cleanup_reraises_at_the_deadline(self) -> None:
        case = InteropCase()
        case._temporary.cleanup()
        temporary = mock.Mock()
        failure = OSError("still busy")
        temporary.cleanup.side_effect = failure
        case._temporary = temporary
        with (
            mock.patch(
                "validation.interop.harness.time.monotonic",
                side_effect=(10.0, 20.0),
            ),
            mock.patch("validation.interop.harness.time.sleep") as sleep,
            self.assertRaises(OSError) as raised,
        ):
            case._cleanup_workspace()
        self.assertIs(raised.exception, failure)
        temporary.cleanup.assert_called_once_with()
        sleep.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "requires Windows file-handle semantics")
    def test_workspace_cleanup_waits_for_a_windows_handle_release(self) -> None:
        case = InteropCase()
        held = (case.work / "held.log").open("wb")
        release = threading.Timer(0.1, held.close)
        release.start()
        try:
            case._cleanup_workspace()
        finally:
            release.join()
            if not held.closed:
                held.close()
            if case.work.exists():
                case._temporary.cleanup()
        self.assertFalse(case.work.exists())

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
