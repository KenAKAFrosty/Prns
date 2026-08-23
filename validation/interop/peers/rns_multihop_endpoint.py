import hashlib
import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


RESOURCE_PAYLOAD_SIZE = 64 * 1024
EXPECTED_HOPS = 3
PEER_TIMEOUT_SECONDS = 90
POLL_SECONDS = 0.05
ANNOUNCE_INTERVAL_SECONDS = 1
PATH_RETRY_SECONDS = 5


def deterministic_payload(label, size):
    seed = label.encode("utf-8")
    blocks = []
    generated = 0
    counter = 0
    while generated < size:
        block = hashlib.sha256(seed + counter.to_bytes(8, "big")).digest()
        blocks.append(block)
        generated += len(block)
        counter += 1
    return b"".join(blocks)[:size]


def configuration(role, port):
    if role == "left":
        interface = (
            "[[Left Endpoint Client]]\n"
            "type = TCPClientInterface\n"
            "enabled = Yes\n"
            "target_host = 127.0.0.1\n"
            f"target_port = {port}\n"
        )
    else:
        interface = (
            "[[Right Endpoint Server]]\n"
            "type = TCPServerInterface\n"
            "enabled = Yes\n"
            "listen_ip = 127.0.0.1\n"
            f"listen_port = {port}\n"
        )
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n" + interface
    )


def prepare_identity(role, path):
    identity = RNS.Identity()
    if not identity.to_file(path):
        raise RuntimeError(f"could not save {role} identity")
    destination_hash = RNS.Destination.hash_from_name_and_identity(
        f"prns.multihop.{role}", identity
    )
    print(destination_hash.hex(), flush=True)


def start_reticulum(role, port):
    config_dir = pathlib.Path(os.environ["RNS_MULTIHOP_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(
        configuration(role, port), encoding="utf-8"
    )
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)


def local_identity(mode):
    if mode != "cold-path":
        return RNS.Identity()
    identity = RNS.Identity.from_file(os.environ["RNS_MULTIHOP_IDENTITY_PATH"])
    if identity is None:
        raise RuntimeError("cold-path identity did not load")
    return identity


def destination_for(identity, role):
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "multihop",
        role,
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    return destination


def await_start():
    start = pathlib.Path(os.environ["RNS_MULTIHOP_START"])
    deadline = time.monotonic() + PEER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if start.exists():
            return
        time.sleep(POLL_SECONDS)
    raise RuntimeError("multihop scenario start was not released")


def outgoing_destination(identity, role):
    return RNS.Destination(
        identity,
        RNS.Destination.OUT,
        RNS.Destination.SINGLE,
        "prns",
        "multihop",
        role,
    )


def run_resources(destination, role, other):
    state = {
        "failure": None,
        "link": None,
        "hops": None,
        "outgoing_complete": False,
        "incoming_complete": False,
    }

    def outgoing_concluded(resource):
        if resource.status != RNS.Resource.COMPLETE:
            state["failure"] = f"outgoing resource failed with status {resource.status}"
            return
        state["outgoing_complete"] = True

    def outgoing_link(link):
        RNS.Resource(
            deterministic_payload(f"multihop-{role}", RESOURCE_PAYLOAD_SIZE),
            link,
            auto_compress=False,
            callback=outgoing_concluded,
        )

    class OtherSeeker:
        aspect_filter = f"prns.multihop.{other}"

        def received_announce(self, destination_hash, announced_identity, app_data):
            del app_data
            if state["link"] is not None:
                return
            hops = RNS.Transport.hops_to(destination_hash)
            if hops != EXPECTED_HOPS:
                state["failure"] = f"expected {EXPECTED_HOPS} path hops, got {hops}"
                return
            state["hops"] = hops
            remote = outgoing_destination(announced_identity, other)
            state["link"] = RNS.Link(remote, established_callback=outgoing_link)

    def incoming_concluded(resource):
        if resource.status != RNS.Resource.COMPLETE:
            state["failure"] = f"incoming resource failed with status {resource.status}"
            return
        data = resource.data.read() if hasattr(resource.data, "read") else resource.data
        expected = deterministic_payload(f"multihop-{other}", RESOURCE_PAYLOAD_SIZE)
        if data != expected:
            state["failure"] = f"incoming resource bytes differed length={len(data)}"
            return
        state["incoming_complete"] = True

    def incoming_link(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(incoming_concluded)

    destination.set_link_established_callback(incoming_link)
    RNS.Transport.register_announce_handler(OtherSeeker())
    deadline = time.monotonic() + PEER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        if state["outgoing_complete"] and state["incoming_complete"]:
            print(
                f"MULTIHOP_OK role={role} hops={state['hops']} bytes={RESOURCE_PAYLOAD_SIZE}",
                flush=True,
            )
            time.sleep(1)
            return
        destination.announce()
        time.sleep(ANNOUNCE_INTERVAL_SECONDS)
    raise RuntimeError(
        f"endpoint timeout role={role} hops={state['hops']} "
        f"outgoing={state['outgoing_complete']} incoming={state['incoming_complete']}"
    )


def run_single(destination, role, other):
    await_start()
    outgoing_payload = f"transport-single-{role}".encode("utf-8")
    incoming_payload = f"transport-single-{other}".encode("utf-8")
    state = {"failure": None, "hops": None, "receipt": None, "received": False}

    def received(payload, packet):
        del packet
        if payload != incoming_payload or state["received"]:
            state["failure"] = f"unexpected transported SINGLE payload {payload!r}"
            return
        state["received"] = True

    class OtherSeeker:
        aspect_filter = f"prns.multihop.{other}"

        def received_announce(self, destination_hash, announced_identity, app_data):
            del app_data
            if state["receipt"] is not None:
                return
            hops = RNS.Transport.hops_to(destination_hash)
            if hops != EXPECTED_HOPS:
                state["failure"] = f"expected {EXPECTED_HOPS} path hops, got {hops}"
                return
            state["hops"] = hops
            receipt = RNS.Packet(
                outgoing_destination(announced_identity, other), outgoing_payload
            ).send()
            if receipt is None or receipt is False:
                state["failure"] = "transported SINGLE send returned no receipt"
                return
            state["receipt"] = receipt

    destination.set_packet_callback(received)
    RNS.Transport.register_announce_handler(OtherSeeker())
    deadline = time.monotonic() + PEER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        receipt = state["receipt"]
        if receipt is not None and receipt.status == RNS.PacketReceipt.FAILED:
            raise RuntimeError("transported SINGLE proof failed")
        if (
            receipt is not None
            and receipt.status == RNS.PacketReceipt.DELIVERED
            and state["received"]
        ):
            print(
                f"TRANSPORT_SINGLE_OK role={role} hops={state['hops']} sent={len(outgoing_payload)} received={len(incoming_payload)} proof=1",
                flush=True,
            )
            time.sleep(1)
            return
        destination.announce()
        time.sleep(ANNOUNCE_INTERVAL_SECONDS)
    raise RuntimeError(
        f"transported SINGLE timeout role={role} hops={state['hops']} "
        f"sent={state['receipt'] is not None} received={state['received']}"
    )


def run_cold_path(destination, role, other):
    await_start()
    remote_hash = bytes.fromhex(os.environ["RNS_MULTIHOP_REMOTE_DESTINATION"])
    outgoing_payload = f"cold-path-{role}".encode("utf-8")
    incoming_payload = f"cold-path-{other}".encode("utf-8")
    state = {"failure": None, "hops": None, "receipt": None, "received": False}

    def received(payload, packet):
        del packet
        if payload != incoming_payload or state["received"]:
            state["failure"] = f"unexpected cold-path payload {payload!r}"
            return
        state["received"] = True

    destination.set_packet_callback(received)
    request_count = 0
    last_request = None

    def request_path():
        nonlocal request_count, last_request
        request_count += 1
        RNS.Transport.request_path(remote_hash)
        last_request = time.monotonic()
        print(f"COLD_PATH_REQUESTED role={role} count={request_count}", flush=True)

    if role == "left":
        request_path()
    deadline = time.monotonic() + PEER_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        if role == "right" and state["received"] and request_count == 0:
            request_path()
        if state["receipt"] is None and RNS.Transport.has_path(remote_hash):
            hops = RNS.Transport.hops_to(remote_hash)
            if hops != EXPECTED_HOPS:
                raise RuntimeError(f"expected {EXPECTED_HOPS} path hops, got {hops}")
            remote_identity = RNS.Identity.recall(remote_hash)
            if remote_identity is None:
                raise RuntimeError(
                    "cold path response did not publish the remote identity"
                )
            state["hops"] = hops
            receipt = RNS.Packet(
                outgoing_destination(remote_identity, other), outgoing_payload
            ).send()
            if receipt is None or receipt is False:
                raise RuntimeError("cold-path send returned no receipt")
            state["receipt"] = receipt
        receipt = state["receipt"]
        if receipt is not None and receipt.status == RNS.PacketReceipt.FAILED:
            raise RuntimeError("cold-path proof failed")
        if (
            receipt is not None
            and receipt.status == RNS.PacketReceipt.DELIVERED
            and state["received"]
        ):
            print(
                f"COLD_PATH_OK role={role} hops={state['hops']} requests={request_count} proof=1",
                flush=True,
            )
            time.sleep(1)
            return
        now = time.monotonic()
        if (
            state["receipt"] is None
            and last_request is not None
            and now - last_request >= PATH_RETRY_SECONDS
        ):
            request_path()
        time.sleep(POLL_SECONDS)
    raise RuntimeError(
        f"cold-path timeout role={role} hops={state['hops']} requests={request_count} "
        f"sent={state['receipt'] is not None} received={state['received']}"
    )


def run_endpoint(mode):
    role = os.environ["RNS_MULTIHOP_ROLE"]
    if role not in ("left", "right"):
        raise RuntimeError(f"unknown endpoint role {role}")
    other = "right" if role == "left" else "left"
    port = int(os.environ["RNS_MULTIHOP_ENDPOINT_PORT"])
    start_reticulum(role, port)
    destination = destination_for(local_identity(mode), role)
    print(
        f"MULTIHOP_ENDPOINT_UP role={role} scenario={mode} destination={destination.hash.hex()}",
        flush=True,
    )
    if mode == "resources":
        run_resources(destination, role, other)
        return
    if mode == "single":
        run_single(destination, role, other)
        return
    if mode == "cold-path":
        run_cold_path(destination, role, other)
        return
    raise RuntimeError(f"unknown multihop endpoint mode {mode}")


def main():
    mode = sys.argv[1]
    if mode == "prepare":
        prepare_identity(sys.argv[2], sys.argv[3])
        return
    run_endpoint(mode)


if __name__ == "__main__":
    sys.exit(main())
