#!/usr/bin/env python3
"""Pinned Python LXMF live peer for the native Prns direct-packet gate."""

from __future__ import annotations

import os
import pathlib
import sys
import time

import LXMF
import RNS


EXPECTED_FROM_RUST = b"rust-to-python"
SENT_FROM_PYTHON = b"python-to-rust"
PEER_SECRET = bytes([0x52]) * 64


def configuration(port: int) -> str:
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[LXMF TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main() -> int:
    port = int(os.environ["PRNS_LXMF_TCP_PORT"])
    config_dir = pathlib.Path(os.environ["PRNS_LXMF_CONFIG_DIR"])
    config_dir.mkdir(parents=True, exist_ok=True)
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
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
        "failure": None,
    }

    def received(message):
        if message.content != EXPECTED_FROM_RUST:
            state["failure"] = f"unexpected Rust content {message.content!r}"
            return
        if message.title != b"Rust":
            state["failure"] = f"unexpected Rust title {message.title!r}"
            return
        if not message.signature_validated:
            state["failure"] = (
                "Rust LXMF source signature was not verified by pinned Python"
            )
            return
        state["received"] = True

    router.register_delivery_callback(received)

    class RustDeliverySeeker:
        aspect_filter = "lxmf.delivery"

        def received_announce(self, destination_hash, announced_identity, app_data):
            del app_data
            if state["rust_destination"] is not None:
                return
            if destination_hash == delivery.hash:
                return
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "lxmf",
                "delivery",
            )
            if remote.hash != destination_hash:
                state["failure"] = "Rust lxmf.delivery announce association mismatched"
                return
            state["rust_destination"] = remote
            state["rust_observed_at"] = time.time()

    RNS.Transport.register_announce_handler(RustDeliverySeeker())
    print(f"PINNED_PYTHON_LXMF_UP {delivery.hash.hex()}", flush=True)
    deadline = time.time() + 45
    last_announce = 0.0
    last_progress = 0.0
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        if time.time() - last_announce >= 0.4:
            router.announce(delivery.hash)
            last_announce = time.time()
        if (
            state["outbound"] is None
            and state["rust_destination"] is not None
            and time.time() - state["rust_observed_at"] >= 1.0
        ):
            outbound = LXMF.LXMessage(
                state["rust_destination"],
                delivery,
                content=SENT_FROM_PYTHON,
                title=b"Python",
                desired_method=LXMF.LXMessage.DIRECT,
            )
            state["outbound"] = outbound
            router.handle_outbound(outbound)
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
