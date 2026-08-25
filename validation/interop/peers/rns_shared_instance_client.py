#!/usr/bin/env python3
import os
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


EXPECTED_APP_DATA = b"prns-shared-server"
PAYLOAD = b"stock-to-prns-shared-server"


class ServerDetector:
    aspect_filter = "personal.smoke"

    def __init__(self):
        self.receipt = None

    def received_announce(self, destination_hash, announced_identity, app_data):
        if app_data != EXPECTED_APP_DATA or self.receipt is not None:
            return
        destination = RNS.Destination(
            announced_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "personal",
            "smoke",
        )
        self.receipt = RNS.Packet(destination, PAYLOAD).send()


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-smoke-")
    instance_port = os.environ.get("PRNS_LOCAL_PORT")
    if instance_port is not None:
        control_port = os.environ.get("PRNS_RPC_PORT", str(int(instance_port) + 1))
        with open(f"{configdir}/config", "w", encoding="utf-8") as config:
            config.write(
                "[reticulum]\n"
                "share_instance = Yes\n"
                "shared_instance_type = tcp\n"
                f"shared_instance_port = {instance_port}\n"
                f"instance_control_port = {control_port}\n"
            )
    start_reference_reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)
    detector = ServerDetector()
    RNS.Transport.register_announce_handler(detector)
    time.sleep(1.5)

    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "personal",
        "smoke",
    )
    print("ANNOUNCED dest=" + destination.hash.hex(), flush=True)

    deadline = time.time() + 20
    while time.time() < deadline:
        destination.announce(app_data=b"stock-shared-client")
        if (
            detector.receipt is not None
            and detector.receipt.get_status() == RNS.PacketReceipt.DELIVERED
        ):
            print(f"STOCK_TO_PRNS_SHARED_OK bytes={len(PAYLOAD)} proof=1", flush=True)
            time.sleep(1)
            return 0
        time.sleep(0.5)
    raise RuntimeError("stock RNS did not deliver application traffic to the Prns shared server")


if __name__ == "__main__":
    sys.exit(main())
