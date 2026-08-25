import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


EXPECTED_FROM_PRNS = b"prns-direct-link-packet"
SENT_FROM_STOCK = b"stock-direct-link-packet"


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Link Packet TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_LINK_PACKET_PORT"])
    config_dir = pathlib.Path(os.environ["PRNS_LINK_PACKET_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "link",
        "packet",
        "stock",
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {
        "received_from_prns": False,
        "outbound_link": None,
        "outbound_receipt": None,
        "reported": False,
        "failure": None,
    }

    def received_from_prns(plaintext, _packet):
        if plaintext != EXPECTED_FROM_PRNS:
            state["failure"] = f"unexpected Prns Link plaintext {plaintext!r}"
            return
        state["received_from_prns"] = True

    def inbound_established(link):
        link.set_packet_callback(received_from_prns)

    def outbound_established(link):
        state["outbound_receipt"] = RNS.Packet(link, SENT_FROM_STOCK).send()

    class PrnsSeeker:
        aspect_filter = "prns.link.packet.interop"

        def received_announce(self, destination_hash, announced_identity, app_data):
            del app_data
            if state["outbound_link"] is not None:
                return
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "link",
                "packet",
                "interop",
            )
            if remote.hash != destination_hash:
                state["failure"] = (
                    "Prns Link destination hash did not match its announce"
                )
                return
            state["outbound_link"] = RNS.Link(
                remote,
                established_callback=outbound_established,
            )

    destination.set_link_established_callback(inbound_established)
    RNS.Transport.register_announce_handler(PrnsSeeker())
    print(f"LINK_PACKET_PEER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        if not state["received_from_prns"]:
            destination.announce()
        receipt = state["outbound_receipt"]
        outbound_proven = (
            receipt is not None and receipt.status == RNS.PacketReceipt.DELIVERED
        )
        if state["received_from_prns"] and outbound_proven and not state["reported"]:
            state["reported"] = True
            print("STOCK_LINK_PACKET_OK received=1 proof=1", flush=True)
            time.sleep(1)
            return 0
        if receipt is not None and receipt.status == RNS.PacketReceipt.FAILED:
            raise RuntimeError("Prns did not prove the stock direct Link packet")
        time.sleep(0.25)
    raise RuntimeError(f"Link packet exchange did not complete state={state!r}")


if __name__ == "__main__":
    sys.exit(main())
