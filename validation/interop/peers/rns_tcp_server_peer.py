#!/usr/bin/env python3
"""Direction-A TCP parity smoke peer: a stock RNS TCPServerInterface hosting ``hopspot.host``.

A standalone stock ``RNS.Reticulum`` whose only interface is a ``TCPServerInterface``. It hosts a
``hopspot.host`` SINGLE destination with ``PROVE_ALL`` and announces it every two seconds, so a Prns
client that dials in hears the announce, sends the destination a single packet, and receives the proof
back over the same wire — the stock-side counterpart of ``rns_tcp_client_peer.py``.

Env: ``PRNS_TCP_LISTEN_PORT`` is the port the stock server listens on.
Prints ``SERVER_UP`` once listening and ``RECEIVED <len>`` for each delivered single.
"""

import os
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

PORT = os.environ["PRNS_TCP_LISTEN_PORT"]

CONFIG = f"""[reticulum]
  enable_transport = No
  share_instance = No
  panic_on_interface_error = No

[logging]
  loglevel = 3

[interfaces]
  [[TCP Server Interface]]
    type = TCPServerInterface
    interface_enabled = True
    listen_ip = 127.0.0.1
    listen_port = {PORT}
"""


def on_packet(data, packet):
    print(f"RECEIVED {len(data)}", flush=True)


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-tcpserver-")
    with open(os.path.join(configdir, "config"), "w") as handle:
        handle.write(CONFIG)
    start_reference_reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)

    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "hopspot",
        "host",
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    destination.set_packet_callback(on_packet)
    print("SERVER_UP", flush=True)

    deadline = time.time() + 90
    while time.time() < deadline:
        destination.announce(app_data=b"stock-tcp-server-host")
        time.sleep(2)
    return 0


if __name__ == "__main__":
    sys.exit(main())
