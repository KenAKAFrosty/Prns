import os
import pathlib
import socket
import sys
import threading
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


INITIAL_PAYLOAD = b"tunnel-route-initial"
RECOVERED_PAYLOAD = b"tunnel-route-recovered"


def relay_configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Tunnel Recovery TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def client_configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Tunnel Recovery TCP Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n"
    )


def start_reticulum(configuration):
    config_dir = pathlib.Path(os.environ["PRNS_TUNNEL_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(configuration, encoding="utf-8")
    return start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)


def relay(port):
    start_reticulum(relay_configuration(port))
    server = next(
        interface
        for interface in RNS.Transport.interfaces
        if getattr(interface, "bind_port", None) == port
        and hasattr(interface, "spawned_interfaces")
    )
    lock = threading.Lock()
    state = {"announce_count": 0, "destination": None, "failure": None}

    class DestinationSeeker:
        aspect_filter = "prns.tunnel.recovery.client"

        def received_announce(self, destination_hash, announced_identity, app_data):
            del app_data
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "tunnel",
                "recovery",
                "client",
            )
            with lock:
                state["announce_count"] += 1
                if remote.hash != destination_hash:
                    state["failure"] = "Prns tunnel destination hash did not match its announce"
                elif state["destination"] is None:
                    state["destination"] = remote

    RNS.Transport.register_announce_handler(DestinationSeeker())
    print("STOCK_TUNNEL_RELAY_UP", flush=True)
    initial_receipt = None
    recovered_receipt = None
    old_interface = None
    tunnel_id = None
    deadline = time.time() + 40
    while time.time() < deadline:
        with lock:
            announce_count = state["announce_count"]
            destination = state["destination"]
            failure = state["failure"]
        if failure is not None:
            raise RuntimeError(failure)
        if announce_count > 1:
            raise RuntimeError(f"fresh endpoint announce observed count={announce_count}")
        if destination is not None and initial_receipt is None:
            initial_receipt = RNS.Packet(destination, INITIAL_PAYLOAD).send()
            if initial_receipt is None:
                raise RuntimeError("initial tunnel packet returned no receipt")
        if initial_receipt is not None and initial_receipt.status == RNS.PacketReceipt.FAILED:
            raise RuntimeError("initial tunnel route packet failed")
        if (
            initial_receipt is not None
            and initial_receipt.status == RNS.PacketReceipt.DELIVERED
            and old_interface is None
        ):
            old_interface = RNS.Transport.next_hop_interface(destination.hash)
            if old_interface is None:
                raise RuntimeError("initial tunnel route has no next-hop interface")
            tunnel_id = getattr(old_interface, "tunnel_id", None)
            if tunnel_id is None:
                raise RuntimeError("initial route was not associated with a tunnel")
            print("STOCK_TUNNEL_RELAY_INITIAL_OK proof=1 announce_count=1", flush=True)
            old_interface.detach()
            old_interface.teardown()
        if old_interface is not None and recovered_receipt is None:
            recovered_interface = RNS.Transport.next_hop_interface(destination.hash)
            if (
                recovered_interface is not None
                and recovered_interface is not old_interface
                and getattr(recovered_interface, "tunnel_id", None) == tunnel_id
            ):
                recovered_receipt = RNS.Packet(destination, RECOVERED_PAYLOAD).send()
                if recovered_receipt is None:
                    raise RuntimeError("recovered tunnel packet returned no receipt")
        if recovered_receipt is not None:
            if recovered_receipt.status == RNS.PacketReceipt.FAILED:
                raise RuntimeError("recovered tunnel route packet failed")
            if recovered_receipt.status == RNS.PacketReceipt.DELIVERED:
                if announce_count != 1:
                    raise RuntimeError(
                        f"recovery used a fresh endpoint announce count={announce_count}"
                    )
                print(
                    "STOCK_TUNNEL_RELAY_OK proof=2 announce_count=1 tunnel_reappeared=1",
                    flush=True,
                )
                time.sleep(1)
                return 0
        time.sleep(0.05)
    raise RuntimeError("stock relay tunnel recovery timed out")


def client(port):
    start_reticulum(client_configuration(port))
    interface = next(
        candidate
        for candidate in RNS.Transport.interfaces
        if getattr(candidate, "initiator", False)
        and getattr(candidate, "target_port", None) == port
    )
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "tunnel",
        "recovery",
        "stock",
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    lock = threading.Lock()
    state = {
        "failure": None,
        "initial": False,
        "initial_proof_baseline": None,
        "recovered": False,
        "recovered_proof_baseline": None,
    }

    def received(payload, packet):
        with lock:
            if payload == INITIAL_PAYLOAD and not state["initial"]:
                state["initial"] = True
                state["initial_proof_baseline"] = packet.receiving_interface.txb
            elif payload == RECOVERED_PAYLOAD and not state["recovered"]:
                state["recovered"] = True
                state["recovered_proof_baseline"] = packet.receiving_interface.txb
            else:
                state["failure"] = f"unexpected tunnel recovery payload {payload!r}"

    destination.set_packet_callback(received)
    print("STOCK_TUNNEL_CLIENT_UP", flush=True)
    destination.announce(app_data=b"stock-tunnel-recovery")
    print("STOCK_TUNNEL_ANNOUNCED count=1", flush=True)
    disconnected = False
    old_socket = None
    deadline = time.time() + 40
    while time.time() < deadline:
        with lock:
            failure = state["failure"]
            initial = state["initial"]
            initial_baseline = state["initial_proof_baseline"]
            recovered = state["recovered"]
            recovered_baseline = state["recovered_proof_baseline"]
        if failure is not None:
            raise RuntimeError(failure)
        if (
            initial
            and initial_baseline is not None
            and interface.txb > initial_baseline
            and not disconnected
        ):
            old_socket = interface.socket
            old_socket.shutdown(socket.SHUT_RDWR)
            old_socket.close()
            disconnected = True
            print("STOCK_TUNNEL_CLIENT_INITIAL_OK proof=1 announce_count=1", flush=True)
        if (
            disconnected
            and recovered
            and recovered_baseline is not None
            and interface.socket is not old_socket
            and interface.txb > recovered_baseline
        ):
            print(
                "STOCK_TUNNEL_CLIENT_OK received=2 announce_count=1 reconnected=1",
                flush=True,
            )
            time.sleep(1)
            return 0
        time.sleep(0.05)
    raise RuntimeError("stock client tunnel recovery timed out")


def main():
    mode = sys.argv[1]
    port = int(os.environ["PRNS_TUNNEL_PORT"])
    if mode == "relay":
        return relay(port)
    if mode == "client":
        return client(port)
    raise RuntimeError(f"unknown tunnel recovery mode {mode}")


if __name__ == "__main__":
    sys.exit(main())
