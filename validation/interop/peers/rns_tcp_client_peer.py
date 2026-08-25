#!/usr/bin/env python3
"""Direction-B TCP parity smoke: a stock RNS node dials *our* TcpServer.

A standalone stock ``RNS.Reticulum`` running only a ``TCPClientInterface`` pointed at the Prns
``rns_interop_peer tcp-server`` scenario. It hears the host's ``hopspot.host`` destination announce (proving our
server carries an announce *outbound* over a stock RNS client link), sends that destination a single
packet (inbound data over the same link), and confirms the packet is *proven* (our ProveAll
destination's proof carried back outbound). One proven round trip exercises our ``TcpServer`` in both
directions against stock RNS's ``TCPClientInterface`` — the inverse of ``local-transit-smoke`` (which
already proves our ``TcpClient`` against stock RNS's ``TCPServerInterface``).

Env: ``PRNS_TCP_TARGET`` is the ``host:port`` of the Prns TcpServer to dial.
Prints ``CLIENT_UP``, ``HEARD_HOST <hex>``, ``PROVEN``, or ``FAILED ...``. Exits 0 once proven.
"""

import os
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

TARGET = os.environ["PRNS_TCP_TARGET"]
HOST, PORT = TARGET.rsplit(":", 1)

CONFIG = f"""[reticulum]
  enable_transport = No
  share_instance = No
  panic_on_interface_error = No

[logging]
  loglevel = 3

[interfaces]
  [[TCP Client Interface]]
    type = TCPClientInterface
    interface_enabled = True
    target_host = {HOST}
    target_port = {PORT}
"""


class HostSeeker:
    aspect_filter = "hopspot.host"

    def __init__(self):
        self.sent = False
        self.proven = False

    def received_announce(self, destination_hash, announced_identity, app_data):
        print("HEARD_HOST " + RNS.prettyhexrep(destination_hash), flush=True)
        if self.sent:
            return
        self.sent = True
        destination = RNS.Destination(
            announced_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "hopspot",
            "host",
        )
        packet = RNS.Packet(destination, b"prns-tcp-parity-ping")
        receipt = packet.send()
        if receipt:
            receipt.set_delivery_callback(self.on_proven)
        else:
            print("FAILED send returned no receipt", flush=True)

    def on_proven(self, receipt):
        print("PROVEN", flush=True)
        self.proven = True


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-tcpclient-")
    with open(os.path.join(configdir, "config"), "w") as handle:
        handle.write(CONFIG)
    start_reference_reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)
    print("CLIENT_UP", flush=True)

    seeker = HostSeeker()
    RNS.Transport.register_announce_handler(seeker)

    deadline = time.time() + 30
    while time.time() < deadline:
        if seeker.proven:
            return 0
        time.sleep(0.2)

    print(f"FAILED proven={seeker.proven} sent={seeker.sent}", flush=True)
    return 1


if __name__ == "__main__":
    sys.exit(main())
