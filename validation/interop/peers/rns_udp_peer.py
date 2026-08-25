import os
import pathlib
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum




def main():
    local_port = int(os.environ["RNS_UDP_LOCAL_PORT"])
    peer_port = int(os.environ["RNS_UDP_PEER_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix="rns-udp-peer-"))
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[UDP Peer]]\n"
        "type = UDPInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {local_port}\n"
        "forward_ip = 127.0.0.1\n"
        f"forward_port = {peer_port}\n",
        encoding="utf-8",
    )
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "udp",
        "stock",
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {
        "received": False,
        "proven": False,
        "sent": False,
        "receipt": None,
        "failure": None,
    }

    def received(data, packet):
        if data != b"prns-udp-proof":
            state["failure"] = f"unexpected Prns UDP payload {data!r}"
            return
        state["received"] = True

    def proven(receipt):
        state["proven"] = True

    class PrnsSeeker:
        aspect_filter = "prns.udp.client"

        def received_announce(self, destination_hash, announced_identity, app_data):
            if state["sent"]:
                return
            state["sent"] = True
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "udp",
                "client",
            )
            receipt = RNS.Packet(remote, b"stock-udp-proof").send()
            if receipt is None:
                state["failure"] = "stock UDP send returned no receipt"
                return
            state["receipt"] = receipt
            receipt.set_delivery_callback(proven)

    destination.set_packet_callback(received)
    RNS.Transport.register_announce_handler(PrnsSeeker())
    print(f"UDP_PEER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 30
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        receipt = state["receipt"]
        if receipt is not None and receipt.get_status() == RNS.PacketReceipt.DELIVERED:
            state["proven"] = True
        if state["received"] and state["proven"]:
            print("STOCK_UDP_OK received=1 proven=1", flush=True)
            time.sleep(0.5)
            return 0
        destination.announce()
        time.sleep(0.5)
    raise RuntimeError(
        f"UDP timeout received={state['received']} proven={state['proven']} sent={state['sent']}"
    )


if __name__ == "__main__":
    sys.exit(main())
