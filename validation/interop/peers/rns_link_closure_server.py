import os
import pathlib
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


READY_MESSAGE_TYPE = 0x1339


class ReadyMessage(RNS.MessageBase):
    MSGTYPE = READY_MESSAGE_TYPE

    def __init__(self, payload=b""):
        self.payload = payload

    def pack(self):
        return self.payload

    def unpack(self, raw):
        self.payload = raw


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Link Closure TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_LINK_CLOSURE_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix="rns-link-closure-server-"))
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    prns_closes = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "link",
        "close",
        "prns",
    )
    stock_closes = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "link",
        "close",
        "stock",
    )
    state = {
        "first_link": None,
        "first_ready": False,
        "first_closed": False,
        "second_link": None,
        "second_ready": False,
        "second_closed": False,
    }

    def ready_handler(expected, field):
        def receive(message):
            if message.payload != expected:
                raise RuntimeError(f"unexpected readiness payload {message.payload!r}")
            state[field] = True
            return True

        return receive

    def first_closed(link):
        if link.teardown_reason != RNS.Link.INITIATOR_CLOSED:
            raise RuntimeError(f"unexpected first Link close reason {link.teardown_reason}")
        if not state["first_ready"]:
            raise RuntimeError("Prns closed the first Link before proven readiness traffic")
        state["first_closed"] = True
        print("STOCK_OBSERVED_PRNS_CLOSE reason=initiator", flush=True)

    def first_established(link):
        if state["first_link"] is not None:
            raise RuntimeError("Prns established the first closure Link more than once")
        state["first_link"] = link
        link.set_link_closed_callback(first_closed)
        channel = link.get_channel()
        channel.register_message_type(ReadyMessage)
        channel.add_message_handler(
            ready_handler(b"prns-ready-to-close", "first_ready")
        )

    def second_established(link):
        if state["second_link"] is not None:
            raise RuntimeError("Prns established the second closure Link more than once")
        state["second_link"] = link
        channel = link.get_channel()
        channel.register_message_type(ReadyMessage)
        channel.add_message_handler(
            ready_handler(b"prns-ready-for-stock-close", "second_ready")
        )

    prns_closes.set_link_established_callback(first_established)
    stock_closes.set_link_established_callback(second_established)
    print("LINK_CLOSURE_SERVER_UP", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["first_link"] is None:
            prns_closes.announce()
        elif state["first_closed"] and state["second_link"] is None:
            stock_closes.announce()
        if state["second_ready"] and not state["second_closed"]:
            state["second_link"].teardown()
            state["second_closed"] = True
            print("STOCK_CLOSED_PRNS_LINK reason=destination", flush=True)
        time.sleep(0.05)
    raise RuntimeError(f"Link closure exchange did not complete state={state!r}")


if __name__ == "__main__":
    sys.exit(main())
