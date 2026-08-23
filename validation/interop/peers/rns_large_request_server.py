import os
import pathlib
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


RESPONSE_SIZE = 128 * 1024


STOCK_RESPONSE = bytes((index * 17 + 3) % 256 for index in range(RESPONSE_SIZE))
EXPECTED_PRNS_RESPONSE = bytes((index * 29 + 7) % 256 for index in range(RESPONSE_SIZE))


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Large Request TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_LARGE_REQUEST_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix="rns-large-request-server-"))
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "large",
        "stock",
    )
    state = {"link": None, "receipt": None, "failure": None, "reported": False}

    def stock_response(path, data, request_id, link_id, remote_identity, requested_at):
        if data != b"prns-request":
            state["failure"] = f"unexpected Prns request payload {data!r}"
            return None
        return STOCK_RESPONSE

    def linked(link):
        state["receipt"] = link.request(
            "/large",
            data=b"stock-request",
            timeout=30,
        )

    class PrnsSeeker:
        aspect_filter = "prns.large.client"

        def received_announce(self, destination_hash, announced_identity, app_data):
            if state["link"] is not None:
                return
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "large",
                "client",
            )
            state["link"] = RNS.Link(remote, established_callback=linked)
            destination.announce()

    destination.register_request_handler(
        "/large",
        response_generator=stock_response,
        allow=RNS.Destination.ALLOW_ALL,
        auto_compress=False,
    )
    RNS.Transport.register_announce_handler(PrnsSeeker())
    print(f"LARGE_REQUEST_SERVER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        receipt = state["receipt"]
        if receipt is not None and receipt.concluded() and not state["reported"]:
            response = receipt.get_response()
            if response != EXPECTED_PRNS_RESPONSE:
                raise RuntimeError(
                    f"unexpected Prns response length={None if response is None else len(response)}"
                )
            state["reported"] = True
            print(f"STOCK_LARGE_REQUEST_OK response={len(response)}", flush=True)
        time.sleep(0.05)
    if state["reported"]:
        return 0
    raise RuntimeError("stock large request did not conclude")


if __name__ == "__main__":
    sys.exit(main())
