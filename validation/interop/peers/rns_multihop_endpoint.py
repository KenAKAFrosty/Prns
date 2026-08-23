import hashlib
import os
import pathlib
import sys
import tempfile
import time

import RNS


PAYLOAD_SIZE = 64 * 1024
EXPECTED_HOPS = 3


def deterministic_payload(role):
    seed = f"multihop-{role}".encode("utf-8")
    blocks = []
    generated = 0
    counter = 0
    while generated < PAYLOAD_SIZE:
        block = hashlib.sha256(seed + counter.to_bytes(8, "big")).digest()
        blocks.append(block)
        generated += len(block)
        counter += 1
    return b"".join(blocks)[:PAYLOAD_SIZE]


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
        "[interfaces]\n"
        + interface
    )


def main():
    role = os.environ["RNS_MULTIHOP_ROLE"]
    if role not in ("left", "right"):
        raise RuntimeError(f"unknown endpoint role {role}")
    other = "right" if role == "left" else "left"
    port = int(os.environ["RNS_MULTIHOP_ENDPOINT_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix=f"rns-multihop-{role}-"))
    config_dir.joinpath("config").write_text(configuration(role, port), encoding="utf-8")
    RNS.Reticulum(configdir=str(config_dir), loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "multihop",
        role,
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
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
            deterministic_payload(role),
            link,
            auto_compress=False,
            callback=outgoing_concluded,
        )

    class OtherSeeker:
        aspect_filter = f"prns.multihop.{other}"

        def received_announce(self, destination_hash, announced_identity, app_data):
            if state["link"] is not None:
                return
            hops = RNS.Transport.hops_to(destination_hash)
            if hops != EXPECTED_HOPS:
                state["failure"] = f"expected {EXPECTED_HOPS} path hops, got {hops}"
                return
            state["hops"] = hops
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "multihop",
                other,
            )
            state["link"] = RNS.Link(remote, established_callback=outgoing_link)

    def incoming_concluded(resource):
        if resource.status != RNS.Resource.COMPLETE:
            state["failure"] = f"incoming resource failed with status {resource.status}"
            return
        data = resource.data.read() if hasattr(resource.data, "read") else resource.data
        if data != deterministic_payload(other):
            state["failure"] = f"incoming resource bytes differed length={len(data)}"
            return
        state["incoming_complete"] = True

    def incoming_link(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(incoming_concluded)

    destination.set_link_established_callback(incoming_link)
    RNS.Transport.register_announce_handler(OtherSeeker())
    print(f"MULTIHOP_ENDPOINT_UP role={role} destination={destination.hash.hex()}", flush=True)
    deadline = time.time() + 90
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        if state["outgoing_complete"] and state["incoming_complete"]:
            print(
                f"MULTIHOP_OK role={role} hops={state['hops']} bytes={PAYLOAD_SIZE}",
                flush=True,
            )
            time.sleep(1)
            return 0
        destination.announce()
        time.sleep(1)
    raise RuntimeError(
        f"endpoint timeout role={role} hops={state['hops']} "
        f"outgoing={state['outgoing_complete']} incoming={state['incoming_complete']}"
    )


if __name__ == "__main__":
    sys.exit(main())
