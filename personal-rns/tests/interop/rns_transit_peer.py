#!/usr/bin/env python3
"""Real-RNS transit smoke: the remote peer behind the Prns bridge (TCP side).

A standalone stock ``RNS.Reticulum`` (reference 1.3.1) running only a TCP server interface. The Prns
bridge daemon dials this server, so this peer sits on the bridge's *network* side, opposite the local
client. It hosts a destination (``prns.peer``), announces it across the bridge, accepts an inbound link
the client establishes through the bridge, and also links *back* to the client's own destination
(``prns.client``) to send it a multi-part resource (an image-sized transfer) — exercising transit in
both directions, the network-to-local-client direction being the one a shared instance must carry
inward to an app, and the one whose shallow egress lane used to shed resource parts under the burst.

Prints ``PEER_DEST <hex>`` once, ``RESOURCE_OK <len>`` when an inbound resource completes, and
``LINK_OUT_UP`` when its own link to the client goes active. Exits 0 if it received, non-zero on
timeout. RNS's own logs go to stderr.

Env: ``PEER_TCP_PORT`` is the loopback port the TCP server listens on (the bridge dials it).
"""

import os
import sys
import tempfile
import time

import RNS

PORT = int(os.environ["PEER_TCP_PORT"])

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


class ClientSeeker:
    aspect_filter = "prns.client"

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
            "client",
        )
        self.link = RNS.Link(destination, established_callback=self.on_up)

    def on_up(self, link):
        print("LINK_OUT_UP", flush=True)
        RNS.Resource(os.urandom(1000000), link, auto_compress=False)


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-peer-")
    with open(os.path.join(configdir, "config"), "w") as handle:
        handle.write(CONFIG)
    RNS.Reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)

    identity = RNS.Identity()
    mine = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "prns", "peer"
    )
    mine.set_proof_strategy(RNS.Destination.PROVE_ALL)

    received = {"hit": False}

    def resource_concluded(resource):
        if resource.status == RNS.Resource.COMPLETE:
            data = resource.data.read() if hasattr(resource.data, "read") else resource.data
            print("RESOURCE_OK " + str(len(data)), flush=True)
            received["hit"] = True
        else:
            print("RESOURCE_FAIL status=" + str(resource.status), flush=True)

    def link_established(link):
        print("LINK_IN", flush=True)
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(resource_concluded)

    mine.set_link_established_callback(link_established)
    print("PEER_DEST " + mine.hash.hex(), flush=True)

    RNS.Transport.register_announce_handler(ClientSeeker())

    deadline = time.time() + 60
    while time.time() < deadline and not received["hit"]:
        mine.announce()
        time.sleep(1.0)

    time.sleep(1.0)
    return 0 if received["hit"] else 3


if __name__ == "__main__":
    sys.exit(main())
