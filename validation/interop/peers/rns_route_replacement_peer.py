import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


TARGET_NAME = "prns.route.replacement.target"
PAYLOAD = b"route-replacement-proof"
INITIAL_HOPS = 3
REPLACEMENT_HOPS = 2
PEER_TIMEOUT_SECONDS = 90
POLL_SECONDS = 0.05
ANNOUNCE_INTERVAL_SECONDS = 0.75


def endpoint_configuration(label, port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        f"[[Route Replacement {label} Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def requester_configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Route Replacement Requester Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n"
    )


def start_reticulum(configuration):
    config_dir = pathlib.Path(os.environ["RNS_ROUTE_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(configuration, encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)


def prepare_identity(path):
    identity = RNS.Identity()
    if not identity.to_file(path):
        raise RuntimeError("could not save route-replacement identity")
    destination_hash = RNS.Destination.hash_from_name_and_identity(
        TARGET_NAME, identity
    )
    print(destination_hash.hex(), flush=True)


def load_identity():
    identity = RNS.Identity.from_file(os.environ["RNS_ROUTE_IDENTITY_PATH"])
    if identity is None:
        raise RuntimeError("route-replacement identity did not load")
    return identity


def target_destination(identity, direction):
    return RNS.Destination(
        identity,
        direction,
        RNS.Destination.SINGLE,
        "prns",
        "route",
        "replacement",
        "target",
    )


def run_endpoint(label, port):
    start_reticulum(endpoint_configuration(label, port))
    server = next(
        interface
        for interface in RNS.Transport.interfaces
        if getattr(interface, "bind_port", None) == port
        and hasattr(interface, "spawned_interfaces")
    )
    destination = target_destination(load_identity(), RNS.Destination.IN)
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {"failure": None, "received": False}

    def received(payload, packet):
        del packet
        if payload != PAYLOAD or state["received"]:
            state["failure"] = f"unexpected route-replacement payload {payload!r}"
            return
        state["received"] = True
        if label == "long":
            print("LONG_ROUTE_USED", flush=True)

    destination.set_packet_callback(received)
    announce_trigger = pathlib.Path(os.environ["RNS_ROUTE_ANNOUNCE_TRIGGER"])
    stop_trigger = (
        pathlib.Path(os.environ["RNS_ROUTE_STOP_TRIGGER"]) if label == "long" else None
    )
    verify_trigger = (
        pathlib.Path(os.environ["RNS_ROUTE_VERIFY_TRIGGER"])
        if label == "long"
        else None
    )
    print(
        f"ROUTE_ENDPOINT_UP role={label} destination={destination.hash.hex()}",
        flush=True,
    )
    announce_count = 0
    stopped = False
    next_announce = time.monotonic()
    deadline = time.monotonic() + PEER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        if label == "long" and state["received"]:
            raise RuntimeError("replacement payload used the incumbent long route")
        if label == "short" and state["received"]:
            print(
                f"SHORT_ROUTE_RECEIVED bytes={len(PAYLOAD)} announces={announce_count}",
                flush=True,
            )
            time.sleep(1)
            return
        if verify_trigger is not None and verify_trigger.exists():
            connected = [
                interface
                for interface in server.spawned_interfaces
                if interface.online and not interface.detached
            ]
            if len(connected) != 1:
                raise RuntimeError(
                    f"long route connection was not alive count={len(connected)}"
                )
            print("LONG_ROUTE_CONNECTED count=1", flush=True)
            time.sleep(1)
            return
        if stop_trigger is not None and stop_trigger.exists():
            if not stopped:
                stopped = True
                print("LONG_ROUTE_SILENT", flush=True)
        now = time.monotonic()
        if announce_trigger.exists() and not stopped and now >= next_announce:
            destination.announce(app_data=f"route-{label}".encode("utf-8"))
            announce_count += 1
            next_announce = now + ANNOUNCE_INTERVAL_SECONDS
        time.sleep(POLL_SECONDS)
    raise RuntimeError(
        f"route endpoint timeout role={label} announces={announce_count} received={state['received']}"
    )


def run_requester(port):
    start_reticulum(requester_configuration(port))
    expected_hash = bytes.fromhex(os.environ["RNS_ROUTE_DESTINATION"])
    state = {
        "failure": None,
        "initial_hops": None,
        "replacement_hops": None,
        "receipt": None,
    }

    class TargetSeeker:
        aspect_filter = TARGET_NAME

        def received_announce(self, destination_hash, announced_identity, app_data):
            del app_data
            if destination_hash != expected_hash:
                state["failure"] = "route-replacement destination hash changed"
                return
            hops = RNS.Transport.hops_to(destination_hash)
            if state["initial_hops"] is None:
                if hops != INITIAL_HOPS:
                    state["failure"] = (
                        f"expected initial route at {INITIAL_HOPS} hops, got {hops}"
                    )
                    return
                state["initial_hops"] = hops
                print(f"STOCK_ROUTE_INITIAL hops={hops}", flush=True)
                return
            if state["receipt"] is not None or hops >= state["initial_hops"]:
                return
            if hops != REPLACEMENT_HOPS:
                state["failure"] = (
                    f"expected replacement route at {REPLACEMENT_HOPS} hops, got {hops}"
                )
                return
            state["replacement_hops"] = hops
            receipt = RNS.Packet(
                target_destination(announced_identity, RNS.Destination.OUT), PAYLOAD
            ).send()
            if receipt is None or receipt is False:
                state["failure"] = "route-replacement send returned no receipt"
                return
            state["receipt"] = receipt

    RNS.Transport.register_announce_handler(TargetSeeker())
    print("ROUTE_REQUESTER_UP", flush=True)
    deadline = time.monotonic() + PEER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        receipt = state["receipt"]
        if receipt is not None and receipt.status == RNS.PacketReceipt.FAILED:
            raise RuntimeError("route-replacement proof failed")
        if receipt is not None and receipt.status == RNS.PacketReceipt.DELIVERED:
            print(
                f"STOCK_ROUTE_REPLACEMENT_OK initial_hops={state['initial_hops']} replacement_hops={state['replacement_hops']} proof=1",
                flush=True,
            )
            time.sleep(1)
            return
        time.sleep(POLL_SECONDS)
    raise RuntimeError(
        f"route requester timeout initial={state['initial_hops']} "
        f"replacement={state['replacement_hops']} sent={state['receipt'] is not None}"
    )


def main():
    mode = sys.argv[1]
    if mode == "prepare":
        prepare_identity(sys.argv[2])
        return
    port = int(os.environ["RNS_ROUTE_PORT"])
    if mode in ("long", "short"):
        run_endpoint(mode, port)
        return
    if mode == "requester":
        run_requester(port)
        return
    raise RuntimeError(f"unknown route-replacement mode {mode}")


if __name__ == "__main__":
    sys.exit(main())
