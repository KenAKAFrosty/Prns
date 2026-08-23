#!/usr/bin/env python3
"""Real-RNS transit smoke: the remote peer behind the Prns bridge (TCP side).

A standalone stock ``RNS.Reticulum`` (pinned reference RNS) running only a TCP server interface. The Prns
bridge daemon dials this server, so this peer sits on the bridge's *network* side, opposite the local
client. It hosts a destination (``prns.peer``), announces it across the bridge, accepts an inbound link
the client establishes through the bridge, and also links *back* to the client's own destination
(``prns.client``) to send it a multi-part resource (an image-sized transfer) — exercising transit in
both directions, the network-to-local-client direction being the one a shared instance must carry
inward to an app, and the one whose shallow egress lane used to shed resource parts under the burst.

Prints ``PEER_DEST <hex>`` once, ``RESOURCE_OK <len>`` when an inbound resource completes, and
``LINK_OUT_UP`` when its own link to the client goes active. RNS's own logs go to stderr.

Env: ``PEER_TCP_PORT`` is the loopback port the TCP server listens on (the bridge dials it).
"""

import os
import sys
import tempfile
import threading
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

PORT = int(os.environ["PEER_TCP_PORT"])
RESOURCE_BYTES = 1_000_000
TRANSFER_TIMEOUT_SECONDS = 120
IFAC_NETWORK_NAME = os.environ.get("PRNS_IFAC_NETWORK_NAME", "")
IFAC_PASSPHRASE = os.environ.get("PRNS_IFAC_PASSPHRASE", "")
IFAC_SIZE_BYTES = int(os.environ.get("PRNS_IFAC_SIZE_BYTES", "16"))
IFAC_CONFIG = ""
if IFAC_NETWORK_NAME:
    IFAC_CONFIG += f"    network_name = {IFAC_NETWORK_NAME}\n"
if IFAC_PASSPHRASE:
    IFAC_CONFIG += f"    passphrase = {IFAC_PASSPHRASE}\n"
if IFAC_CONFIG:
    IFAC_CONFIG += f"    ifac_size = {IFAC_SIZE_BYTES * 8}\n"

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
{IFAC_CONFIG}
"""


class ClientSeeker:
    aspect_filter = "prns.client"

    def __init__(self, transfer):
        self.transfer = transfer
        self.link = None
        self.link_creation = threading.Lock()

    def received_announce(self, destination_hash, announced_identity, app_data):
        with self.link_creation:
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

        def outgoing_concluded(resource):
            if resource.status == RNS.Resource.COMPLETE:
                self.transfer["outgoing_complete"] = True
                print("RESOURCE_SENT_OK " + str(RESOURCE_BYTES), flush=True)
            else:
                self.transfer["failure"] = f"outgoing resource status={resource.status}"
                print("RESOURCE_SEND_FAIL status=" + str(resource.status), flush=True)

        RNS.Resource(
            os.urandom(RESOURCE_BYTES),
            link,
            auto_compress=False,
            callback=outgoing_concluded,
        )


class HostileDetector:
    aspect_filter = "prns.hostile"

    def received_announce(self, destination_hash, announced_identity, app_data):
        print("HOSTILE_RECEIVED", flush=True)


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-peer-")
    with open(os.path.join(configdir, "config"), "w") as handle:
        handle.write(CONFIG)
    start_reference_reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)

    identity = RNS.Identity()
    mine = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "prns", "peer"
    )
    mine.set_proof_strategy(RNS.Destination.PROVE_ALL)

    transfer = {
        "incoming_complete": False,
        "outgoing_complete": False,
        "failure": None,
    }

    def resource_concluded(resource):
        if resource.status == RNS.Resource.COMPLETE:
            data = resource.data.read() if hasattr(resource.data, "read") else resource.data
            print("RESOURCE_OK " + str(len(data)), flush=True)
            transfer["incoming_complete"] = True
        else:
            transfer["failure"] = f"incoming resource status={resource.status}"
            print("RESOURCE_FAIL status=" + str(resource.status), flush=True)

    def link_established(link):
        print("LINK_IN", flush=True)
        link.track_phy_stats(True)
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(resource_concluded)

    mine.set_link_established_callback(link_established)
    print("PEER_DEST " + mine.hash.hex(), flush=True)

    RNS.Transport.register_announce_handler(ClientSeeker(transfer))
    RNS.Transport.register_announce_handler(HostileDetector())

    deadline = time.monotonic() + TRANSFER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if transfer["failure"] is not None:
            return 3
        if transfer["incoming_complete"] and transfer["outgoing_complete"]:
            time.sleep(1.0)
            return 0
        mine.announce()
        time.sleep(1.0)

    print(
        "RESOURCE_TIMEOUT "
        f"incoming={int(transfer['incoming_complete'])} "
        f"outgoing={int(transfer['outgoing_complete'])}",
        flush=True,
    )
    return 3


if __name__ == "__main__":
    sys.exit(main())
