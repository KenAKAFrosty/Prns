#!/usr/bin/env python3

import os
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

PORT = os.environ["PRNS_TCP_LISTEN_PORT"]
EXPECTED_FROM_PRNS = b"prns-tcp-parity-ping"

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
    if data != EXPECTED_FROM_PRNS:
        print(f"RECEIVED_MISMATCH len={len(data)}", flush=True)
        return
    print("STOCK_TCP_SERVER_OK received=1", flush=True)


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
