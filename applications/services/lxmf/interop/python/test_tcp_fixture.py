"""Safety tests for development LXMF TCP fixture configuration."""

from __future__ import annotations

import contextlib
import io
import pathlib
from types import SimpleNamespace
import unittest
from unittest import mock

import LXMF

import live_tcp_lxmf_peer
import run_physical_tcp_lxmf
from live_tcp_lxmf_peer import (
    OUTBOUND_PROOF_MARKER,
    OUTBOUND_SUBMITTED_MARKER,
    RustDeliverySeeker,
    receive_rust_message,
    report_outbound_marker,
    report_outbound_proof,
    report_rust_observed,
    source_verification_failure,
    submit_outbound_if_due,
)
from run_live_tcp_lxmf import host_environment
from run_physical_tcp_lxmf import arguments
from tcp_fixture import (
    EXPECTED_DESTINATION_ENV,
    LISTEN_IP_ENV,
    OUTBOUND_DELAY_ENV,
    RUST_OBSERVED_MARKER,
    WILDCARD_OPT_IN_ENV,
    environment_expected_destination,
    environment_listen_ip,
    environment_outbound_delay,
    server_configuration,
    tcp_target,
)


class TcpFixtureTests(unittest.TestCase):
    def test_host_environment_does_not_inherit_physical_peer_selection(self) -> None:
        inherited = {
            EXPECTED_DESTINATION_ENV: "0123456789abcdef0123456789abcdef",
            OUTBOUND_DELAY_ENV: "nan",
            LISTEN_IP_ENV: "0.0.0.0",
            WILDCARD_OPT_IN_ENV: "1",
            "PRNS_LXMF_TCP_PORT": "4242",
            "PRNS_LXMF_CONFIG_DIR": "/physical/rns",
            "CARGO_TARGET_DIR": "/existing/target",
        }
        before = inherited.copy()
        environment = host_environment(inherited, pathlib.Path("/host-fixture"), 5678)
        self.assertNotIn(EXPECTED_DESTINATION_ENV, environment)
        self.assertIsNone(environment_expected_destination(environment))
        self.assertNotIn(OUTBOUND_DELAY_ENV, environment)
        self.assertEqual(environment_outbound_delay(environment, 45), 1.0)
        self.assertNotIn(WILDCARD_OPT_IN_ENV, environment)
        self.assertEqual(environment_listen_ip(environment), "127.0.0.1")
        self.assertEqual(environment["PRNS_LXMF_TCP_PORT"], "5678")
        self.assertEqual(environment["PRNS_LXMF_CONFIG_DIR"], "/host-fixture/rns")
        self.assertEqual(environment["PYTHONIOENCODING"], "utf-8:strict")
        self.assertEqual(environment["CARGO_TARGET_DIR"], "/existing/target")
        self.assertEqual(inherited, before)

    def test_listen_defaults_to_loopback(self) -> None:
        self.assertEqual(environment_listen_ip({}), "127.0.0.1")

    def test_wildcard_bind_requires_exact_opt_in(self) -> None:
        with self.assertRaisesRegex(ValueError, WILDCARD_OPT_IN_ENV):
            environment_listen_ip({LISTEN_IP_ENV: "0.0.0.0"})
        with self.assertRaisesRegex(ValueError, WILDCARD_OPT_IN_ENV):
            environment_listen_ip(
                {LISTEN_IP_ENV: "0.0.0.0", WILDCARD_OPT_IN_ENV: "true"}
            )
        self.assertEqual(
            environment_listen_ip(
                {LISTEN_IP_ENV: "0.0.0.0", WILDCARD_OPT_IN_ENV: "1"}
            ),
            "0.0.0.0",
        )

    def test_configuration_uses_only_validated_address_and_port(self) -> None:
        configuration = server_configuration(4242, "192.0.2.10")
        self.assertIn("listen_ip = 192.0.2.10\n", configuration)
        self.assertIn("listen_port = 4242\n", configuration)
        with self.assertRaisesRegex(ValueError, "explicit IPv4 or IPv6"):
            server_configuration(4242, "127.0.0.1\n[[Injected]]")
        with self.assertRaisesRegex(ValueError, "1 through 65535"):
            server_configuration(0, "127.0.0.1")

    def test_targets_are_concrete_and_ipv6_is_bracketed(self) -> None:
        self.assertEqual(tcp_target("192.0.2.10", 4242), "192.0.2.10:4242")
        self.assertEqual(tcp_target("2001:db8::1", 4242), "[2001:db8::1]:4242")
        with self.assertRaisesRegex(ValueError, "concrete"):
            tcp_target("::", 4242)

    def test_physical_launcher_requires_wildcard_opt_in_and_advertise_ip(self) -> None:
        for unsafe_arguments in (
            ["--listen-ip", "0.0.0.0"],
            ["--listen-ip", "0.0.0.0", "--allow-wildcard-bind"],
        ):
            with self.subTest(arguments=unsafe_arguments):
                with contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit):
                        arguments(unsafe_arguments)

        selected = arguments(
            [
                "--listen-ip",
                "0.0.0.0",
                "--allow-wildcard-bind",
                "--advertise-ip",
                "192.0.2.10",
                "--port",
                "4242",
            ]
        )
        self.assertTrue(selected.listen_address.is_unspecified)
        self.assertEqual(str(selected.advertise_address), "192.0.2.10")

    def test_physical_launcher_help_requires_observed_marker_before_send(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            with self.assertRaises(SystemExit) as stopped:
                arguments(["--help"])
        self.assertEqual(stopped.exception.code, 0)
        help_text = output.getvalue()
        normalized_help = " ".join(help_text.split())
        self.assertIn("announce the local lxmf.delivery destination", normalized_help)
        self.assertIn(RUST_OBSERVED_MARKER, normalized_help)
        self.assertIn("then send the Rust test message", normalized_help)

    def test_expected_destination_environment_requires_exact_hex_or_absence(self) -> None:
        self.assertIsNone(environment_expected_destination({}))
        destination = "0123456789abcdef0123456789ABCDEF"
        self.assertEqual(
            environment_expected_destination({EXPECTED_DESTINATION_ENV: destination}),
            bytes.fromhex(destination),
        )
        for invalid in ("", "a" * 31, "a" * 33, "g" * 32, " " + "a" * 31, "a" * 31 + "\n"):
            with self.subTest(value=invalid):
                with self.assertRaisesRegex(ValueError, EXPECTED_DESTINATION_ENV):
                    environment_expected_destination({EXPECTED_DESTINATION_ENV: invalid})

    def test_physical_launcher_validates_and_normalizes_expected_destination(self) -> None:
        destination = "0123456789ABCDEF0123456789ABCDEF"
        self.assertEqual(
            arguments(["--expected-destination", destination]).expected_destination,
            destination.lower(),
        )
        self.assertIsNone(arguments([]).expected_destination)
        for invalid in ("", "a" * 31, "a" * 33, "g" * 32, " " + "a" * 31, "a" * 31 + "\n"):
            with self.subTest(value=invalid):
                with contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as stopped:
                        arguments(["--expected-destination", invalid])
                self.assertEqual(stopped.exception.code, 2)

    def test_outbound_delay_environment_is_finite_and_below_exchange_timeout(self) -> None:
        self.assertEqual(environment_outbound_delay({}, 60), 1.0)
        self.assertEqual(environment_outbound_delay({OUTBOUND_DELAY_ENV: "12.5"}, 60), 12.5)
        for invalid in ("", "other", "nan", "inf", "-inf", "-1", "0", "0.999", "60", "61"):
            with self.subTest(value=invalid):
                with self.assertRaisesRegex(ValueError, OUTBOUND_DELAY_ENV):
                    environment_outbound_delay({OUTBOUND_DELAY_ENV: invalid}, 60)

    def test_physical_launcher_validates_outbound_delay_against_its_timeout(self) -> None:
        self.assertEqual(arguments([]).outbound_delay_seconds, 1.0)
        self.assertEqual(
            arguments(["--timeout-seconds", "60", "--outbound-delay-seconds", "12.5"]).outbound_delay_seconds,
            12.5,
        )
        for invalid in ("other", "nan", "inf", "-inf", "-1", "0", "0.999", "60", "61"):
            with self.subTest(value=invalid):
                with contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as stopped:
                        arguments(["--timeout-seconds", "60", f"--outbound-delay-seconds={invalid}"])
                self.assertEqual(stopped.exception.code, 2)

    def test_physical_launcher_passes_only_explicit_expected_destination(self) -> None:
        destination = "0123456789ABCDEF0123456789ABCDEF"
        for selected, delay in ((None, None), (destination, 12.5)):
            with self.subTest(expected_destination=selected, outbound_delay=delay):
                argv = ["--port", "4242"]
                if selected is not None:
                    argv.extend(["--expected-destination", selected])
                if delay is not None:
                    argv.extend(["--outbound-delay-seconds", str(delay)])
                process = mock.Mock()
                process.wait.return_value = 0
                process.poll.return_value = 0
                with (
                    mock.patch.dict(
                        run_physical_tcp_lxmf.os.environ,
                        {EXPECTED_DESTINATION_ENV: "f" * 32, OUTBOUND_DELAY_ENV: "999"},
                        clear=True,
                    ),
                    mock.patch.object(run_physical_tcp_lxmf, "available_port") as available,
                    mock.patch.object(
                        run_physical_tcp_lxmf, "pinned_python", return_value=pathlib.Path("/pinned/python")
                    ),
                    mock.patch.object(run_physical_tcp_lxmf.tempfile, "TemporaryDirectory") as temporary,
                    mock.patch.object(
                        run_physical_tcp_lxmf.subprocess, "Popen", return_value=process
                    ) as launch,
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    temporary.return_value.__enter__.return_value = "/fixture-test"
                    self.assertEqual(run_physical_tcp_lxmf.main(argv), 0)
                available.assert_not_called()
                environment = launch.call_args.kwargs["env"]
                self.assertEqual(
                    environment_expected_destination(environment),
                    None if selected is None else bytes.fromhex(selected),
                )
                if selected is not None:
                    self.assertEqual(environment[EXPECTED_DESTINATION_ENV], destination.lower())
                self.assertEqual(environment["PRNS_LXMF_TCP_PORT"], "4242")
                self.assertEqual(environment["PRNS_LXMF_CONFIG_DIR"], "/fixture-test/rns")
                self.assertEqual(
                    environment_outbound_delay(environment, 300),
                    1.0 if delay is None else delay,
                )

    def test_peer_rejects_invalid_expected_destination_before_network_or_state(self) -> None:
        with (
            mock.patch.dict(
                live_tcp_lxmf_peer.os.environ,
                {"PRNS_LXMF_TCP_PORT": "4242", EXPECTED_DESTINATION_ENV: "invalid"},
                clear=True,
            ),
            mock.patch.object(live_tcp_lxmf_peer.pathlib, "Path") as path,
            mock.patch.object(live_tcp_lxmf_peer.RNS, "Reticulum") as reticulum,
        ):
            with self.assertRaisesRegex(ValueError, EXPECTED_DESTINATION_ENV):
                live_tcp_lxmf_peer.main()
        path.assert_not_called()
        reticulum.assert_not_called()

    def test_peer_rejects_invalid_outbound_delay_before_network_or_state(self) -> None:
        with (
            mock.patch.dict(
                live_tcp_lxmf_peer.os.environ,
                {"PRNS_LXMF_TCP_PORT": "4242", OUTBOUND_DELAY_ENV: "nan"},
                clear=True,
            ),
            mock.patch.object(live_tcp_lxmf_peer.pathlib, "Path") as path,
            mock.patch.object(live_tcp_lxmf_peer.RNS, "Reticulum") as reticulum,
        ):
            with self.assertRaisesRegex(ValueError, OUTBOUND_DELAY_ENV):
                live_tcp_lxmf_peer.main()
        path.assert_not_called()
        reticulum.assert_not_called()


class PeerProtocolTests(unittest.TestCase):
    def test_outbound_waits_for_selected_peer_and_delay_then_submits_once(self) -> None:
        state = {"outbound": None, "rust_destination": None, "rust_observed_at": None}
        remote = object()
        local = object()
        outbound = SimpleNamespace(hash=None)
        message_hash = bytes(range(32))
        output = io.StringIO()

        def handle_outbound(message):
            self.assertIs(message, outbound)
            self.assertEqual(output.getvalue(), "")
            message.hash = message_hash

        router = SimpleNamespace(handle_outbound=mock.Mock(side_effect=handle_outbound))
        with (
            mock.patch.object(live_tcp_lxmf_peer.LXMF, "LXMessage", return_value=outbound) as message,
            mock.patch.object(live_tcp_lxmf_peer.time, "time", return_value=1000.125),
        ):
            self.assertFalse(submit_outbound_if_due(state, local, router, 30, now=100, stream=output))
            state["rust_destination"] = remote
            state["rust_observed_at"] = 100.0
            self.assertFalse(submit_outbound_if_due(state, local, router, 30, now=129.999, stream=output))
            message.assert_not_called()
            self.assertTrue(submit_outbound_if_due(state, local, router, 30, now=130, stream=output))
            self.assertFalse(submit_outbound_if_due(state, local, router, 30, now=131, stream=output))
            self.assertFalse(report_outbound_marker(state, OUTBOUND_SUBMITTED_MARKER, stream=output))
            message.assert_called_once_with(
                remote, local, content=b"python-to-rust", title=b"Python", desired_method=message.DIRECT
            )
        router.handle_outbound.assert_called_once_with(outbound)
        self.assertEqual(
            output.getvalue(),
            f"{OUTBOUND_SUBMITTED_MARKER} timestamp=1970-01-01T00:16:40.125Z message_hash={message_hash.hex()}\n",
        )

    def test_proof_marker_requires_delivered_state_and_reports_exact_hash_once(self) -> None:
        state = {"outbound": None}
        output = io.StringIO()
        self.assertFalse(report_outbound_proof(state, stream=output))
        outbound = SimpleNamespace(hash=bytes(range(32)), state=LXMF.LXMessage.OUTBOUND)
        state["outbound"] = outbound
        for status in (
            LXMF.LXMessage.OUTBOUND,
            LXMF.LXMessage.SENT,
            LXMF.LXMessage.FAILED,
            LXMF.LXMessage.REJECTED,
            LXMF.LXMessage.CANCELLED,
        ):
            outbound.state = status
            self.assertFalse(report_outbound_proof(state, stream=output))
        self.assertEqual(output.getvalue(), "")
        outbound.state = LXMF.LXMessage.DELIVERED
        with mock.patch.object(live_tcp_lxmf_peer.time, "time", return_value=1001.25):
            self.assertTrue(report_outbound_proof(state, stream=output))
            self.assertFalse(report_outbound_proof(state, stream=output))
        self.assertEqual(
            output.getvalue(),
            f"{OUTBOUND_PROOF_MARKER} timestamp=1970-01-01T00:16:41.250Z message_hash={outbound.hash.hex()}\n",
        )

    def test_outbound_marker_refuses_missing_or_truncated_message_hash(self) -> None:
        for invalid in (None, bytes(16), "00" * 32):
            with self.subTest(message_hash=invalid):
                state = {"outbound": SimpleNamespace(hash=invalid)}
                output = io.StringIO()
                with self.assertRaisesRegex(RuntimeError, "packed 32-byte hash"):
                    report_outbound_marker(state, OUTBOUND_SUBMITTED_MARKER, stream=output)
                self.assertEqual(output.getvalue(), "")
                self.assertNotIn(OUTBOUND_SUBMITTED_MARKER, state)

    def test_expected_receiver_ignores_other_sources_before_payload_checks(self) -> None:
        expected = bytes([2]) * 16
        state = {"received": False, "failure": None}
        for message in (
            SimpleNamespace(source_hash=bytes([3]) * 16),
            SimpleNamespace(),
            SimpleNamespace(
                source_hash=bytes([3]) * 16,
                content=b"rust-to-python",
                title=b"Rust",
                signature_validated=True,
            ),
        ):
            with self.subTest(message=message):
                receive_rust_message(state, message, expected)
                self.assertEqual(state, {"received": False, "failure": None})

    def test_expected_receiver_still_requires_test_payload_and_valid_signature(self) -> None:
        expected = bytes([2]) * 16
        for changed_fields, failure in (
            ({}, None),
            ({"content": b"other"}, "unexpected Rust content"),
            ({"title": b"other"}, "unexpected Rust title"),
            (
                {"signature_validated": False, "unverified_reason": LXMF.LXMessage.SIGNATURE_INVALID},
                "source signature was invalid",
            ),
        ):
            with self.subTest(changed_fields=changed_fields):
                state = {"received": False, "failure": None}
                fields = {
                    "source_hash": expected,
                    "content": b"rust-to-python",
                    "title": b"Rust",
                    "signature_validated": True,
                }
                fields.update(changed_fields)
                receive_rust_message(state, SimpleNamespace(**fields), expected)
                self.assertEqual(state["received"], failure is None)
                if failure is None:
                    self.assertIsNone(state["failure"])
                else:
                    self.assertIn(failure, state["failure"])

    def test_default_receiver_retains_host_message_behavior_without_source_filter(self) -> None:
        state = {"received": False, "failure": None}
        receive_rust_message(
            state,
            SimpleNamespace(content=b"rust-to-python", title=b"Rust", signature_validated=True),
        )
        self.assertEqual(state, {"received": True, "failure": None})
        state = {"received": False, "failure": None}
        receive_rust_message(state, SimpleNamespace(content=b"other"))
        self.assertEqual(state, {"received": False, "failure": "unexpected Rust content b'other'"})

    def seeker_state(self) -> dict[str, object]:
        return {
            "rust_destination": None,
            "rust_observed_at": None,
            "rust_observed_reported": False,
            "failure": None,
        }

    def test_expected_seeker_ignores_other_announces_and_selects_only_expected_peer(self) -> None:
        local = bytes([1]) * 16
        expected = bytes([2]) * 16
        unrelated = bytes([3]) * 16
        state = self.seeker_state()
        seeker = RustDeliverySeeker(state, local, expected)
        remote = SimpleNamespace(hash=expected)
        identity = object()
        output = io.StringIO()
        with (
            mock.patch.object(live_tcp_lxmf_peer.RNS, "Destination", return_value=remote) as destination,
            mock.patch.object(live_tcp_lxmf_peer.time, "time", return_value=123.0),
            contextlib.redirect_stdout(output),
        ):
            seeker.received_announce(local, object(), None)
            seeker.received_announce(unrelated, object(), None)
            destination.assert_not_called()
            self.assertEqual(state, self.seeker_state())
            self.assertEqual(output.getvalue(), "")
            seeker.received_announce(expected, identity, None)
            self.assertIs(state["rust_destination"], remote)
            self.assertEqual(state["rust_observed_at"], 123.0)
            self.assertIsNone(state["failure"])
            seeker.received_announce(unrelated, object(), None)
            seeker.received_announce(expected, identity, None)
            destination.assert_called_once_with(
                identity, destination.OUT, destination.SINGLE, "lxmf", "delivery"
            )
        self.assertEqual(output.getvalue(), f"{RUST_OBSERVED_MARKER} {expected.hex()}\n")

    def test_default_seeker_retains_first_non_self_host_peer_behavior(self) -> None:
        local = bytes([1]) * 16
        first = bytes([2]) * 16
        remote = SimpleNamespace(hash=first)
        state = self.seeker_state()
        seeker = RustDeliverySeeker(state, local)
        with (
            mock.patch.object(live_tcp_lxmf_peer.RNS, "Destination", return_value=remote) as destination,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            seeker.received_announce(local, object(), None)
            destination.assert_not_called()
            seeker.received_announce(first, object(), None)
            seeker.received_announce(bytes([3]) * 16, object(), None)
            destination.assert_called_once()
        self.assertIs(state["rust_destination"], remote)
        self.assertIsNone(state["failure"])

    def test_expected_seeker_preserves_announce_identity_association_check(self) -> None:
        expected = bytes([2]) * 16
        state = self.seeker_state()
        seeker = RustDeliverySeeker(state, bytes([1]) * 16, expected)
        with mock.patch.object(
            live_tcp_lxmf_peer.RNS, "Destination", return_value=SimpleNamespace(hash=bytes([3]) * 16)
        ):
            seeker.received_announce(expected, object(), None)
        self.assertIsNone(state["rust_destination"])
        self.assertIsNone(state["rust_observed_at"])
        self.assertFalse(state["rust_observed_reported"])
        self.assertEqual(state["failure"], "Rust lxmf.delivery announce association mismatched")

    def test_verification_failure_distinguishes_pinned_lxmf_reasons(self) -> None:
        self.assertIsNone(
            source_verification_failure(
                SimpleNamespace(signature_validated=True, unverified_reason=None)
            )
        )

        source_unknown = source_verification_failure(
            SimpleNamespace(
                signature_validated=False,
                unverified_reason=LXMF.LXMessage.SOURCE_UNKNOWN,
            )
        )
        self.assertIn("source identity was unknown", source_unknown)
        self.assertIn(RUST_OBSERVED_MARKER, source_unknown)

        invalid_signature = source_verification_failure(
            SimpleNamespace(
                signature_validated=False,
                unverified_reason=LXMF.LXMessage.SIGNATURE_INVALID,
            )
        )
        self.assertIn("signature was invalid", invalid_signature)
        self.assertNotIn("identity was unknown", invalid_signature)

    def test_verification_failure_defensively_reports_unknown_reason(self) -> None:
        self.assertEqual(
            source_verification_failure(
                SimpleNamespace(signature_validated=False, unverified_reason=99)
            ),
            "Rust LXMF source signature was not verified by pinned Python "
            "(unverified_reason=99)",
        )
        self.assertEqual(
            source_verification_failure(SimpleNamespace(signature_validated=False)),
            "Rust LXMF source signature was not verified by pinned Python "
            "(unverified_reason=missing)",
        )

    def test_rust_observed_marker_is_deterministic_and_emitted_once(self) -> None:
        state: dict[str, object] = {"rust_observed_reported": False}
        output = io.StringIO()
        destination = bytes(range(16))

        self.assertTrue(report_rust_observed(state, destination, stream=output))
        self.assertFalse(report_rust_observed(state, destination, stream=output))
        self.assertEqual(
            output.getvalue(),
            f"{RUST_OBSERVED_MARKER} {destination.hex()}\n",
        )


if __name__ == "__main__":
    unittest.main()
