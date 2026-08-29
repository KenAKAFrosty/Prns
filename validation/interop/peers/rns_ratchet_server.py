import os
import pathlib
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


EXPECTED_FROM_PRNS = [b"prns-ratchet-zero", b"prns-ratchet-one"]
SENT_FROM_STOCK = b"stock-ratchet-proof"


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Ratchet TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_RATCHET_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix="rns-ratchet-server-"))
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "ratchet",
        "stock",
    )
    ratchet_path = config_dir.joinpath("stock.ratchets")
    destination.enable_ratchets(str(ratchet_path))
    destination.enforce_ratchets()
    destination.set_ratchet_interval(1)
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {
        "received": [],
        "ratchet_ids": [],
        "failure": None,
        "initial_ratchet": None,
        "rotate_at": None,
        "rotated": False,
        "prns_proven": False,
        "receipt": None,
        "sent_to_prns": False,
    }

    def received(data, packet):
        index = len(state["received"])
        if index >= len(EXPECTED_FROM_PRNS) or data != EXPECTED_FROM_PRNS[index]:
            state["failure"] = f"unexpected packet index={index} payload={data!r}"
            return
        ratchet_id = None if packet.ratchet_id is None else bytes(packet.ratchet_id)
        if ratchet_id is None:
            state["failure"] = f"packet index={index} was not decrypted with a ratchet"
            return
        if ratchet_id in state["ratchet_ids"]:
            state["failure"] = f"packet index={index} reused ratchet {ratchet_id.hex()}"
            return
        state["ratchet_ids"].append(ratchet_id)
        state["received"].append(data)

    def proven(receipt):
        state["prns_proven"] = True

    class PrnsSeeker:
        aspect_filter = "prns.ratchet.client"

        def received_announce(self, destination_hash, announced_identity, app_data):
            if state["sent_to_prns"]:
                return
            state["sent_to_prns"] = True
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "ratchet",
                "client",
            )
            receipt = RNS.Packet(remote, SENT_FROM_STOCK).send()
            if receipt is None:
                state["failure"] = "stock ratcheted send returned no receipt"
                return
            state["receipt"] = receipt
            receipt.set_delivery_callback(proven)
            destination.announce()
            state["initial_ratchet"] = ratchet_path.read_bytes()
            state["rotate_at"] = time.time() + 1.2

    destination.set_packet_callback(received)
    RNS.Transport.register_announce_handler(PrnsSeeker())
    print(f"RATCHET_SERVER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        receipt = state["receipt"]
        if receipt is not None and receipt.get_status() == RNS.PacketReceipt.DELIVERED:
            state["prns_proven"] = True
        if (
            state["rotate_at"] is not None
            and not state["rotated"]
            and time.time() >= state["rotate_at"]
        ):
            destination.announce()
            rotated_ratchet = ratchet_path.read_bytes()
            if rotated_ratchet == state["initial_ratchet"]:
                raise RuntimeError("stock ratchet did not rotate before the second announce")
            state["rotated"] = True
        if (
            state["received"] == EXPECTED_FROM_PRNS
            and state["rotated"]
            and state["prns_proven"]
        ):
            print(
                "STOCK_RATCHET_OK received=2 distinct_ratchets=2 prns_proven=1",
                flush=True,
            )
            time.sleep(1)
            return 0
        time.sleep(0.05)
    raise RuntimeError(
        f"ratchet timeout received={len(state['received'])} "
        f"rotated={state['rotated']} prns_proven={state['prns_proven']}"
    )


if __name__ == "__main__":
    sys.exit(main())
