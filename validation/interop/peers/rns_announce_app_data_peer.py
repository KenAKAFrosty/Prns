import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


EXPECTED_FROM_PRNS = bytes([0x00, 0x70, 0x72, 0x6E, 0x73, 0xFF])
SENT_FROM_STOCK = bytes([0xFF, 0x73, 0x74, 0x6F, 0x63, 0x6B, 0x00])


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Announce App Data TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_ANNOUNCE_APP_DATA_PORT"])
    config_dir = pathlib.Path(os.environ["PRNS_ANNOUNCE_APP_DATA_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "announce",
        "appdata",
        "stock",
    )
    state = {"received": False, "failure": None}

    class PrnsAnnounceHandler:
        aspect_filter = "prns.announce.appdata.interop"

        def received_announce(self, destination_hash, announced_identity, app_data):
            remote = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "prns",
                "announce",
                "appdata",
                "interop",
            )
            if remote.hash != destination_hash:
                state["failure"] = "Prns destination hash did not match its announce"
                return
            if app_data != EXPECTED_FROM_PRNS:
                state["failure"] = f"unexpected Prns announce app data {app_data!r}"
                return
            state["received"] = True

    RNS.Transport.register_announce_handler(PrnsAnnounceHandler())
    print("ANNOUNCE_APP_DATA_PEER_UP", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        if state["failure"] is not None:
            raise RuntimeError(state["failure"])
        destination.announce(app_data=SENT_FROM_STOCK)
        if state["received"]:
            print("STOCK_ANNOUNCE_APP_DATA_OK received=1", flush=True)
            time.sleep(1)
            return 0
        time.sleep(0.5)
    raise RuntimeError("Prns announce application data was not observed")


if __name__ == "__main__":
    sys.exit(main())
