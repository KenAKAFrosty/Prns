#!/usr/bin/env python3
"""Real-RNS transit smoke: the local client in front of the Prns bridge.

A stock ``RNS.Reticulum`` (pinned reference RNS) that connects to the Prns bridge as a shared-instance
client over the loopback port (forced to TCP so the path is identical on every platform). It has no
interfaces of its own: everything it reaches, it reaches through the bridge. It hosts a destination
(``prns.client``), announces it across the bridge, links to the remote peer (``prns.peer``) and sends
it a multi-part resource over that link, and accepts the peer's link *back* to it. This is the path
LXMF uses for direct messages with attachments, exercised both ways through the bridge.

Prints ``CLIENT_DEST <hex>``, ``LINK_OUT_UP`` when its link to the peer goes active, and
``RESOURCE_OK <len>`` when the peer's inbound resource completes. RNS's own logs go to stderr.

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
  rpc_key = 5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a
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
        RNS.Resource(os.urandom(1000000), link, auto_compress=False)


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

    def resource_concluded(resource):
        if resource.status == RNS.Resource.COMPLETE:
            data = resource.data.read() if hasattr(resource.data, "read") else resource.data
            print("RESOURCE_OK " + str(len(data)), flush=True)
            received["hit"] = True
        else:
            print("RESOURCE_FAIL status=" + str(resource.status), flush=True)

    def link_established(link):
        print("LINK_IN", flush=True)
        link.track_phy_stats(True)
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(resource_concluded)

    mine.set_link_established_callback(link_established)
    print("CLIENT_DEST " + mine.hash.hex(), flush=True)

    RNS.Transport.register_announce_handler(PeerSeeker())

    deadline = time.time() + 60
    while time.time() < deadline and not received["hit"]:
        mine.announce()
        time.sleep(1.0)

    time.sleep(1.0)
    return 0 if received["hit"] else 4


if __name__ == "__main__":
    sys.exit(main())
