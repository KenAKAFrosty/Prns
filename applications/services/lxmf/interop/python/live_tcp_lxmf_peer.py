#!/usr/bin/env python3
"""Pinned Python LXMF live peer for the native Prns direct-packet gate."""

from __future__ import annotations

import os
import pathlib
import sys
import time
from datetime import datetime, timezone

import LXMF
import RNS

from tcp_fixture import (
    RUST_OBSERVED_MARKER,
    environment_expected_destination,
    environment_listen_ip,
    environment_outbound_delay,
    server_configuration,
)


EXPECTED_FROM_RUST = b"rust-to-python"
SENT_FROM_PYTHON = b"python-to-rust"
PEER_SECRET = bytes([0x52]) * 64
OUTBOUND_SUBMITTED_MARKER = "PINNED_PYTHON_LXMF_OUTBOUND_SUBMITTED"
OUTBOUND_PROOF_MARKER = "PINNED_PYTHON_LXMF_OUTBOUND_PROOF"
PATH_REQUEST_MARKER = "PINNED_PYTHON_LXMF_PATH_REQUEST"
PATH_REQUEST_INTERVAL_SECONDS = 10.0


_MISSING_REASON = object()


def source_verification_failure(message) -> str | None:
    """Describe pinned LXMF verification failure without conflating its reasons."""
    if getattr(message, "signature_validated", False):
        return None

    reason = getattr(message, "unverified_reason", _MISSING_REASON)
    source_unknown = getattr(LXMF.LXMessage, "SOURCE_UNKNOWN", _MISSING_REASON)
    signature_invalid = getattr(LXMF.LXMessage, "SIGNATURE_INVALID", _MISSING_REASON)
    if source_unknown is not _MISSING_REASON and reason == source_unknown:
        return (
            "Rust LXMF source identity was unknown to pinned Python; announce the "
            "app's lxmf.delivery destination and wait for a line beginning with "
            f"{RUST_OBSERVED_MARKER} before sending"
        )
    if signature_invalid is not _MISSING_REASON and reason == signature_invalid:
        return "Rust LXMF source signature was invalid according to pinned Python"

    rendered_reason = "missing" if reason is _MISSING_REASON else repr(reason)
    return (
        "Rust LXMF source signature was not verified by pinned Python "
        f"(unverified_reason={rendered_reason})"
    )


def report_rust_observed(
    state: dict[str, object], destination_hash: bytes, *, stream=None
) -> bool:
    """Emit the physical-automation readiness marker at most once."""
    if state.get("rust_observed_reported") is True:
        return False
    state["rust_observed_reported"] = True
    output = sys.stdout if stream is None else stream
    print(f"{RUST_OBSERVED_MARKER} {destination_hash.hex()}", file=output, flush=True)
    return True


def report_outbound_marker(state: dict[str, object], marker: str, *, stream=None) -> bool:
    """Log each outbound milestone once, using the packed LXMF message hash."""
    if state.get(marker) is True:
        return False
    message_hash = state["outbound"].hash
    if not isinstance(message_hash, bytes) or len(message_hash) != 32:
        raise RuntimeError("outbound LXMF message did not expose its packed 32-byte hash")
    timestamp = datetime.fromtimestamp(time.time(), timezone.utc).isoformat(timespec="milliseconds")
    timestamp = timestamp.replace("+00:00", "Z")
    output = sys.stdout if stream is None else stream
    print(f"{marker} timestamp={timestamp} message_hash={message_hash.hex()}", file=output, flush=True)
    state[marker] = True
    return True


def submit_outbound_if_due(
    state: dict[str, object], delivery, router, delay_seconds: float, *, now: float, stream=None
) -> bool:
    if (
        state["outbound"] is not None
        or state["rust_destination"] is None
        or now - state["rust_observed_at"] < delay_seconds
    ):
        return False
    outbound = LXMF.LXMessage(
        state["rust_destination"],
        delivery,
        content=SENT_FROM_PYTHON,
        title=b"Python",
        desired_method=LXMF.LXMessage.DIRECT,
    )
    state["outbound"] = outbound
    router.handle_outbound(outbound)
    # Submission packs the message; it is not evidence of a radio transmission.
    report_outbound_marker(state, OUTBOUND_SUBMITTED_MARKER, stream=stream)
    return True


def report_outbound_proof(state: dict[str, object], *, stream=None) -> bool:
    outbound = state["outbound"]
    if outbound is None or outbound.state != LXMF.LXMessage.DELIVERED:
        return False
    return report_outbound_marker(state, OUTBOUND_PROOF_MARKER, stream=stream)


def receive_rust_message(
    state: dict[str, object], message, expected_destination: bytes | None = None
) -> None:
    if (
        expected_destination is not None
        and getattr(message, "source_hash", None) != expected_destination
    ):
        return
    if message.content != EXPECTED_FROM_RUST:
        state["failure"] = f"unexpected Rust content {message.content!r}"
        return
    if message.title != b"Rust":
        state["failure"] = f"unexpected Rust title {message.title!r}"
        return
    verification_failure = source_verification_failure(message)
    if verification_failure is not None:
        state["failure"] = verification_failure
        return
    state["received"] = True


def request_expected_path_if_due(
    state: dict[str, object], expected_destination: bytes | None, *, deadline: float, stream=None
) -> bool:
    """Discover only an explicitly selected peer, within the monotonic exchange deadline."""
    if (
        expected_destination is None
        or state["rust_destination"] is not None
        or state["failure"] is not None
    ):
        return False
    now = time.monotonic()
    if now >= deadline:
        return False
    last_request = state.get("last_path_request_at")
    if last_request is not None and now - last_request < PATH_REQUEST_INTERVAL_SECONDS:
        return False
    RNS.Transport.request_path(expected_destination)
    state["last_path_request_at"] = now
    timestamp = datetime.fromtimestamp(time.time(), timezone.utc).isoformat(timespec="milliseconds")
    timestamp = timestamp.replace("+00:00", "Z")
    output = sys.stdout if stream is None else stream
    print(
        f"{PATH_REQUEST_MARKER} timestamp={timestamp} destination={expected_destination.hex()}",
        file=output,
        flush=True,
    )
    return True


class RustDeliverySeeker:
    """Select the expected app, or the first non-self peer for isolated host gates."""

    aspect_filter = "lxmf.delivery"

    def __init__(
        self,
        state: dict[str, object],
        local_destination: bytes,
        expected_destination: bytes | None = None,
    ) -> None:
        self.state = state
        self.local_destination = local_destination
        self.expected_destination = expected_destination
        self.receive_path_responses = expected_destination is not None

    def received_announce(self, destination_hash, announced_identity, app_data):
        del app_data
        if self.state["rust_destination"] is not None:
            return
        if destination_hash == self.local_destination:
            return
        if (
            self.expected_destination is not None
            and destination_hash != self.expected_destination
        ):
            return
        remote = RNS.Destination(
            announced_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "lxmf",
            "delivery",
        )
        if remote.hash != destination_hash:
            self.state["failure"] = "Rust lxmf.delivery announce association mismatched"
            return
        self.state["rust_destination"] = remote
        self.state["rust_observed_at"] = time.time()
        report_rust_observed(self.state, destination_hash)


def main() -> int:
    port = int(os.environ["PRNS_LXMF_TCP_PORT"])
    listen_ip = environment_listen_ip(os.environ)
    expected_destination = environment_expected_destination(os.environ)
    exchange_timeout_seconds = float(
        os.environ.get("PRNS_LXMF_EXCHANGE_TIMEOUT_SECONDS", "45")
    )
    if not 1 <= exchange_timeout_seconds <= 3600:
        raise RuntimeError(
            "PRNS_LXMF_EXCHANGE_TIMEOUT_SECONDS must be from 1 through 3600"
        )
    outbound_delay_seconds = environment_outbound_delay(os.environ, exchange_timeout_seconds)
    config_dir = pathlib.Path(os.environ["PRNS_LXMF_CONFIG_DIR"])
    config_dir.mkdir(parents=True, exist_ok=True)
    config_dir.joinpath("config").write_text(
        server_configuration(port, listen_ip), encoding="utf-8"
    )
    loglevel = int(os.environ.get("PRNS_LXMF_PYTHON_LOGLEVEL", RNS.LOG_ERROR))
    RNS.Reticulum(configdir=str(config_dir), loglevel=loglevel)

    identity = RNS.Identity.from_bytes(PEER_SECRET)
    router = LXMF.LXMRouter(
        storagepath=str(config_dir / "lxmf"),
        autopeer=False,
        enforce_stamps=False,
    )
    delivery = router.register_delivery_identity(
        identity,
        display_name="Pinned Python LXMF",
        stamp_cost=None,
    )
    if delivery is None:
        raise RuntimeError("Python LXMF did not register its delivery destination")

    state = {
        "received": False,
        "outbound": None,
        "rust_destination": None,
        "rust_observed_at": None,
        "rust_observed_reported": False,
        "failure": None,
    }

    def received(message):
        receive_rust_message(state, message, expected_destination)

    router.register_delivery_callback(received)

    RNS.Transport.register_announce_handler(
        RustDeliverySeeker(state, delivery.hash, expected_destination)
    )
    print(f"PINNED_PYTHON_LXMF_UP {delivery.hash.hex()}", flush=True)
    deadline = time.monotonic() + exchange_timeout_seconds
    last_announce = 0.0
    last_progress = 0.0
    while time.monotonic() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        request_expected_path_if_due(state, expected_destination, deadline=deadline)
        if time.time() - last_announce >= 0.4:
            router.announce(delivery.hash)
            last_announce = time.time()
        submit_outbound_if_due(
            state, delivery, router, outbound_delay_seconds, now=time.time()
        )
        outbound = state["outbound"]
        if (
            outbound is not None
            and os.environ.get("PRNS_LXMF_DEBUG") == "1"
            and time.time() - last_progress >= 1.0
        ):
            print(
                "PINNED_PYTHON_LXMF_PROGRESS "
                f"state={outbound.state} attempts={outbound.delivery_attempts} "
                f"progress={outbound.progress} representation={outbound.representation} "
                f"direct_links={len(router.direct_links)}",
                flush=True,
            )
            last_progress = time.time()
        outbound_delivered = (
            outbound is not None and outbound.state == LXMF.LXMessage.DELIVERED
        )
        report_outbound_proof(state)
        if outbound is not None and outbound.state in (
            LXMF.LXMessage.FAILED,
            LXMF.LXMessage.REJECTED,
            LXMF.LXMessage.CANCELLED,
        ):
            raise RuntimeError(f"Python outbound failed with state {outbound.state}")
        if state["received"] and outbound_delivered:
            print(
                "PINNED_PYTHON_LXMF_OK inbound=verified outbound=proof links=two",
                flush=True,
            )
            time.sleep(0.5)
            return 0
        time.sleep(0.05)
    raise RuntimeError(f"LXMF exchange timed out state={state!r}")


if __name__ == "__main__":
    sys.exit(main())
