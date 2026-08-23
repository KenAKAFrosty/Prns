import os
import pathlib
import sys
import tempfile
import time

import RNS
from RNS.Channel import MessageState
from rns_protocol_evidence import start_reference_reticulum


MESSAGE_TYPE = 0x1337
EXPECTED_FROM_PRNS = [b"prns-channel-zero", b"prns-channel-one"]
SENT_FROM_STOCK = [b"stock-channel-zero", b"stock-channel-one"]


class InteropMessage(RNS.MessageBase):
    MSGTYPE = MESSAGE_TYPE

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
        "[[Channel TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_CHANNEL_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix="rns-channel-server-"))
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "channel",
    )
    state = {"received": [], "failure": None, "channel": None, "outbound": []}

    def receive(message):
        index = len(state["received"])
        if index >= len(EXPECTED_FROM_PRNS) or message.payload != EXPECTED_FROM_PRNS[index]:
            state["failure"] = f"unexpected message index={index} payload={message.payload!r}"
            return True
        state["received"].append(message.payload)
        if state["received"] == EXPECTED_FROM_PRNS:
            print("STOCK_CHANNEL_OK messages=2 ordered=1", flush=True)
        return True

    def established(link):
        channel = link.get_channel()
        state["channel"] = channel
        channel.register_message_type(InteropMessage)
        channel.add_message_handler(receive)
        for payload in SENT_FROM_STOCK:
            deadline = time.time() + 5
            while not channel.is_ready_to_send() and time.time() < deadline:
                time.sleep(0.01)
            if not channel.is_ready_to_send():
                state["failure"] = "stock channel send window did not open"
                return
            state["outbound"].append(channel.send(InteropMessage(payload)))

    destination.set_link_established_callback(established)
    print(f"CHANNEL_SERVER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        outbound_delivered = sum(
            envelope.outlet.get_packet_state(envelope.packet)
            == MessageState.MSGSTATE_DELIVERED
            for envelope in state["outbound"]
        )
        if state["received"] == EXPECTED_FROM_PRNS and outbound_delivered == len(SENT_FROM_STOCK):
            print("STOCK_CHANNEL_ACKNOWLEDGED messages=2", flush=True)
            time.sleep(1)
            return 0
        destination.announce()
        time.sleep(0.5)
    raise RuntimeError(f"timed out with {len(state['received'])} Prns channel messages")


if __name__ == "__main__":
    sys.exit(main())
