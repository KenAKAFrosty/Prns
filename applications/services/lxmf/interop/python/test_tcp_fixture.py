"""Safety tests for development LXMF TCP fixture configuration."""

from __future__ import annotations

import contextlib
import io
from types import SimpleNamespace
import unittest

import LXMF

from live_tcp_lxmf_peer import report_rust_observed, source_verification_failure
from run_physical_tcp_lxmf import arguments
from tcp_fixture import (
    LISTEN_IP_ENV,
    RUST_OBSERVED_MARKER,
    WILDCARD_OPT_IN_ENV,
    environment_listen_ip,
    server_configuration,
    tcp_target,
)


class TcpFixtureTests(unittest.TestCase):
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


class PeerProtocolTests(unittest.TestCase):
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
