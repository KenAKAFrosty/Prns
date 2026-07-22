#!/usr/bin/env python3

import os
import sys
import tempfile
import time

import RNS


PORT = int(os.environ["PEER_TCP_PORT"])
MODE = sys.argv[1]
NETWORK_NAME = os.environ.get("PRNS_IFAC_NETWORK_NAME", "prns-interop")
PASSPHRASE = os.environ.get("PRNS_IFAC_PASSPHRASE", "ifac-parity-secret")
SIZE_BITS = int(os.environ.get("PRNS_IFAC_SIZE_BYTES", "16")) * 8


class PeerDetector:
    aspect_filter = "prns.peer"

    def received_announce(self, destination_hash, announced_identity, app_data):
        print("HOSTILE_PEER_ANNOUNCE", flush=True)
        destination = RNS.Destination(
            announced_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "prns",
            "peer",
        )
        RNS.Link(
            destination,
            established_callback=lambda link: print("HOSTILE_LINK_ACTIVE", flush=True),
        )


def main():
    if MODE == "missing":
        ifac = ""
    elif MODE == "wrong":
        ifac = (
            f"    network_name = {NETWORK_NAME}\n"
            f"    passphrase = {PASSPHRASE}-wrong\n"
            f"    ifac_size = {SIZE_BITS}\n"
        )
    else:
        raise RuntimeError(f"unknown hostile IFAC mode {MODE}")
    configdir = tempfile.mkdtemp(prefix=f"rns-ifac-{MODE}-")
    config = f"""[reticulum]
  enable_transport = No
  share_instance = No
  panic_on_interface_error = No

[logging]
  loglevel = 2

[interfaces]
  [[Hostile TCP Client]]
    type = TCPClientInterface
    interface_enabled = True
    target_host = 127.0.0.1
    target_port = {PORT}
{ifac}"""
    with open(os.path.join(configdir, "config"), "w", encoding="utf-8") as handle:
        handle.write(config)
    RNS.Reticulum(configdir=configdir, loglevel=RNS.LOG_ERROR)
    RNS.Transport.register_announce_handler(PeerDetector())
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
