import os
import pathlib
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


RESOURCE_PAYLOAD = b"stock-resource-that-must-be-rejected" * 4096
READY_MESSAGE_TYPE = 0x1338


class ReadyMessage(RNS.MessageBase):
    MSGTYPE = READY_MESSAGE_TYPE

    def __init__(self, payload=b""):
        self.payload = payload

    def pack(self):
        return self.payload

    def unpack(self, raw):
        self.payload = raw


def server_configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Resource Rejection TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def client_configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Resource Rejection TCP Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n"
    )


def start_reticulum(configuration, prefix):
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix=prefix))
    config_dir.joinpath("config").write_text(configuration, encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)


def reject_prns(port):
    start_reticulum(server_configuration(port), "rns-resource-reject-server-")
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "resource",
        "reject",
        "stock",
    )
    state = {"offers": 0, "published": 0}

    def reject(resource):
        state["offers"] += 1
        return False

    def concluded(resource):
        state["published"] += 1

    def established(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_APP)
        link.set_resource_callback(reject)
        link.set_resource_concluded_callback(concluded)

    def complete(path, data, request_id, link_id, remote_identity, requested_at):
        if data != b"prns-rejection-observed":
            raise RuntimeError(f"unexpected Prns completion payload {data!r}")
        if state != {"offers": 1, "published": 0}:
            raise RuntimeError(f"unexpected stock receiver state {state!r}")
        print("STOCK_REJECTED_PRNS offers=1 published=0", flush=True)
        return b"stock-no-publication"

    destination.set_link_established_callback(established)
    destination.register_request_handler(
        "/complete",
        response_generator=complete,
        allow=RNS.Destination.ALLOW_ALL,
        auto_compress=False,
    )
    print(f"STOCK_REJECTION_SERVER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        destination.announce()
        time.sleep(0.5)
    raise RuntimeError("Prns did not complete the stock rejection exchange")


def send_to_prns(port):
    start_reticulum(client_configuration(port), "rns-resource-reject-client-")
    state = {"link": None, "resource": None, "receipt": None}

    def established(link):
        channel = link.get_channel()
        channel.register_message_type(ReadyMessage)

        def ready(message):
            if message.payload != b"prns-rejection-policy-ready":
                raise RuntimeError(f"unexpected Prns readiness payload {message.payload!r}")
            if state["resource"] is not None:
                raise RuntimeError("Prns sent duplicate rejection readiness")
            state["resource"] = RNS.Resource(
                RESOURCE_PAYLOAD,
                link,
                auto_compress=False,
            )
            return True

        channel.add_message_handler(ready)

    class PrnsSeeker:
        aspect_filter = "prns.resource.reject.interop"

        def received_announce(self, destination_hash, announced_identity, app_data):
            if state["link"] is not None:
                return
            destination = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "resource",
                "reject",
                "interop",
            )
            state["link"] = RNS.Link(destination, established_callback=established)

    RNS.Transport.register_announce_handler(PrnsSeeker())
    print("STOCK_REJECTION_CLIENT_UP", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        resource = state["resource"]
        if resource is not None and resource.status == RNS.Resource.COMPLETE:
            raise RuntimeError("Prns accepted the Resource configured for rejection")
        if resource is not None and resource.status == RNS.Resource.REJECTED:
            if resource.get_progress() != 0:
                raise RuntimeError(
                    f"rejected Resource transferred payload progress={resource.get_progress()}"
                )
            if state["receipt"] is None:
                state["receipt"] = state["link"].request(
                    "/complete",
                    data=b"stock-rejection-observed",
                    timeout=10,
                )
        receipt = state["receipt"]
        if receipt is not None and receipt.concluded():
            if receipt.get_response() != b"prns-no-publication":
                raise RuntimeError(f"unexpected Prns completion response {receipt.get_response()!r}")
            print("STOCK_OBSERVED_PRNS_REJECTION progress=0", flush=True)
            return
        time.sleep(0.05)
    raise RuntimeError("stock sender did not observe Prns Resource rejection")


def main():
    role = os.environ["PRNS_REJECTION_ROLE"]
    port = int(os.environ["PRNS_REJECTION_PORT"])
    if role == "reject-prns":
        reject_prns(port)
        return
    if role == "send-to-prns":
        send_to_prns(port)
        return
    raise RuntimeError(f"unknown rejection role {role!r}")


if __name__ == "__main__":
    sys.exit(main())
