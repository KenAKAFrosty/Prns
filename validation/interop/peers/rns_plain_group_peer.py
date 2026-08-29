import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


GROUP_IDENTITY_SECRET = bytes([0x22]) * 32 + bytes([0x11]) * 32
GROUP_KEY = bytes([0x42]) * 64
EXPECTED_PLAIN = bytes([0xFF, 0x70, 0x72, 0x6E, 0x73, 0x2D, 0x70, 0x6C, 0x61, 0x69, 0x6E, 0x00])
EXPECTED_GROUP = bytes([0x00, 0x70, 0x72, 0x6E, 0x73, 0x2D, 0x67, 0x72, 0x6F, 0x75, 0x70, 0xFF])
SENT_PLAIN = bytes([0x00, 0x73, 0x74, 0x6F, 0x63, 0x6B, 0x2D, 0x70, 0x6C, 0x61, 0x69, 0x6E, 0xFF])
SENT_GROUP = bytes([0xFF, 0x73, 0x74, 0x6F, 0x63, 0x6B, 0x2D, 0x67, 0x72, 0x6F, 0x75, 0x70, 0x00])


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[PLAIN GROUP TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_PLAIN_GROUP_PORT"])
    config_dir = pathlib.Path(os.environ["PRNS_PLAIN_GROUP_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)

    plain_in = RNS.Destination(
        None,
        RNS.Destination.IN,
        RNS.Destination.PLAIN,
        "prns",
        "destination",
        "plain",
    )
    plain_out = RNS.Destination(
        None,
        RNS.Destination.OUT,
        RNS.Destination.PLAIN,
        "prns",
        "destination",
        "plain",
    )
    group_identity = RNS.Identity.from_bytes(GROUP_IDENTITY_SECRET)
    group_in = RNS.Destination(
        group_identity,
        RNS.Destination.IN,
        RNS.Destination.GROUP,
        "prns",
        "destination",
        "group",
    )
    group_in.load_private_key(GROUP_KEY)
    group_out = RNS.Destination(
        group_identity,
        RNS.Destination.OUT,
        RNS.Destination.GROUP,
        "prns",
        "destination",
        "group",
    )
    group_out.load_private_key(GROUP_KEY)
    state = {"plain": False, "group": False, "failure": None}

    def received_plain(data, packet):
        if data != EXPECTED_PLAIN:
            state["failure"] = f"unexpected Prns PLAIN payload {data!r}"
            return
        state["plain"] = True

    def received_group(data, packet):
        if data != EXPECTED_GROUP:
            state["failure"] = f"unexpected Prns GROUP plaintext {data!r}"
            return
        state["group"] = True

    plain_in.set_packet_callback(received_plain)
    group_in.set_packet_callback(received_group)
    print("STOCK_PLAIN_GROUP_PEER_UP", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        RNS.Packet(plain_out, SENT_PLAIN, create_receipt=False).send()
        RNS.Packet(group_out, SENT_GROUP, create_receipt=False).send()
        if state["plain"] and state["group"]:
            print("STOCK_PLAIN_GROUP_OK received_plain=1 received_group=1", flush=True)
            time.sleep(1)
            return 0
        time.sleep(0.3)
    raise RuntimeError(
        f"PLAIN/GROUP timeout plain={state['plain']} group={state['group']}"
    )


if __name__ == "__main__":
    sys.exit(main())
