#!/usr/bin/env python3

import os
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


PORT = int(os.environ["PEER_TCP_PORT"])
MODE = sys.argv[1]
NETWORK_NAME = os.environ.get("PRNS_IFAC_NETWORK_NAME", "prns-interop")
PASSPHRASE = os.environ.get("PRNS_IFAC_PASSPHRASE", "ifac-parity-secret")
SIZE_BITS = int(os.environ.get("PRNS_IFAC_SIZE_BYTES", "16")) * 8


class PeerDetector:
    aspect_filter = "prns.ifac.server"

    def __init__(self):
        self.receipt = None

    def received_announce(self, destination_hash, announced_identity, app_data):
        if not MODE.startswith("matching"):
            print("HOSTILE_PEER_ANNOUNCE", flush=True)
            destination = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "ifac",
                "server",
            )
            RNS.Link(
                destination,
                established_callback=lambda link: print("HOSTILE_LINK_ACTIVE", flush=True),
            )
            return
        if app_data != b"prns-ifac-server" or self.receipt is not None:
            return
        phase = MODE.removeprefix("matching-")
        payload = f"ifac-matching-{phase}".encode("utf-8")
        destination = RNS.Destination(
            announced_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "prns",
            "ifac",
            "server",
        )
        self.receipt = RNS.Packet(destination, payload).send()


def ifac_configuration():
    if MODE == "missing":
        return ""
    if MODE == "wrong":
        passphrase = f"{PASSPHRASE}-wrong"
    elif MODE in ("matching-before", "matching-after"):
        passphrase = PASSPHRASE
    else:
        raise RuntimeError(f"unknown IFAC mode {MODE}")
    return (
        f"    network_name = {NETWORK_NAME}\n"
        f"    passphrase = {passphrase}\n"
        f"    ifac_size = {SIZE_BITS}\n"
    )


def main():
    ifac = ifac_configuration()
    configdir = tempfile.mkdtemp(prefix=f"rns-ifac-{MODE}-")
    config = f"""[reticulum]
  enable_transport = No
  share_instance = No
  panic_on_interface_error = No

[logging]
  loglevel = 2

[interfaces]
  [[IFAC TCP Client]]
    type = TCPClientInterface
    interface_enabled = True
    target_host = 127.0.0.1
    target_port = {PORT}
{ifac}"""
    with open(os.path.join(configdir, "config"), "w", encoding="utf-8") as handle:
        handle.write(config)
    start_reference_reticulum(configdir=configdir, loglevel=RNS.LOG_ERROR)
    detector = PeerDetector()
    RNS.Transport.register_announce_handler(detector)
    if MODE.startswith("matching"):
        deadline = time.time() + 15
        while time.time() < deadline:
            if (
                detector.receipt is not None
                and detector.receipt.get_status() == RNS.PacketReceipt.DELIVERED
            ):
                phase = MODE.removeprefix("matching-")
                print(f"MATCHING_IFAC_OK phase={phase} proof=1", flush=True)
                time.sleep(1)
                return 0
            time.sleep(0.1)
        raise RuntimeError(f"matching IFAC traffic did not complete for {MODE}")
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "hostile",
    )
    time.sleep(0.75)
    destination.announce(app_data=MODE.encode("utf-8"))
    print(f"HOSTILE_SENT {MODE}", flush=True)
    time.sleep(3)
    return 0


if __name__ == "__main__":
    sys.exit(main())
