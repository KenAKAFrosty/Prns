#!/usr/bin/env python3
"""Real-RNS transit smoke: the local client in front of the Prns bridge.

A stock ``RNS.Reticulum`` (reference 1.3.1) that connects to the Prns bridge as a shared-instance
client over the loopback port (forced to TCP so the path is identical on every platform). It has no
interfaces of its own: everything it reaches, it reaches through the bridge. It hosts a destination
(``prns.client``), announces it across the bridge, links to the remote peer (``prns.peer``) and sends
over that link, and accepts the peer's link *back* to it. This is the path LXMF uses for direct
messages, exercised both ways through the bridge.

Prints ``PEER_DEST`` is the peer's; this prints ``CLIENT_DEST <hex>``, ``LINK_OUT_UP`` when its link to
the peer goes active, and ``RECEIVED <text>`` when the peer's link data arrives inbound. RNS's own logs
go to stderr.

Env: ``PRNS_LOCAL_PORT`` is the bridge's loopback shared-instance port.
"""

import os
import sys
import tempfile
import time

import RNS

LOCAL_PORT = int(os.environ["PRNS_LOCAL_PORT"])

CONFIG = f"""[reticulum]
  enable_transport = No
  share_instance = Yes
  shared_instance_type = tcp
  shared_instance_port = {LOCAL_PORT}
  instance_control_port = {LOCAL_PORT + 1}
  panic_on_interface_error = No

[logging]
  loglevel = 3
"""


class PeerSeeker:
    aspect_filter = "prns.peer"

    def __init__(self):
        self.link = None

    def received_announce(self, destination_hash, announced_identity, app_data):
        if self.link is not None:
            return
        destination = RNS.Destination(
            announced_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "prns",
            "peer",
        )
        self.link = RNS.Link(destination, established_callback=self.on_up)

    def on_up(self, link):
        print("LINK_OUT_UP", flush=True)
        RNS.Packet(link, b"client-to-peer").send()


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-client-")
    with open(os.path.join(configdir, "config"), "w") as handle:
        handle.write(CONFIG)
    RNS.Reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)
    time.sleep(1.5)

    identity = RNS.Identity()
    mine = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "prns", "client"
    )
    mine.set_proof_strategy(RNS.Destination.PROVE_ALL)

    received = {"hit": False}

    def link_packet(data, _packet):
        print("RECEIVED " + data.decode("utf-8", "replace"), flush=True)
        received["hit"] = True

    def link_established(link):
        print("LINK_IN", flush=True)
        link.set_packet_callback(link_packet)

    mine.set_link_established_callback(link_established)
    print("CLIENT_DEST " + mine.hash.hex(), flush=True)

    RNS.Transport.register_announce_handler(PeerSeeker())

    deadline = time.time() + 45
    while time.time() < deadline and not received["hit"]:
        mine.announce()
        time.sleep(1.0)

    time.sleep(1.0)
    return 0 if received["hit"] else 4


if __name__ == "__main__":
    sys.exit(main())
