import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


def main():
    listen_port = int(os.environ["RNS_MULTIHOP_LISTEN_PORT"])
    peer_port = int(os.environ["RNS_MULTIHOP_PEER_PORT"])
    config_dir = pathlib.Path(os.environ["RNS_MULTIHOP_CONFIG_DIR"])
    config_dir.mkdir()
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Left Endpoint Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "mode = gateway\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {listen_port}\n"
        "[[Prns Transport Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "recursive_prs = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {peer_port}\n",
        encoding="utf-8",
    )
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    print(
        f"MULTIHOP_TRANSPORT_UP listen=127.0.0.1:{listen_port} peer=127.0.0.1:{peer_port}",
        flush=True,
    )
    while True:
        time.sleep(0.25)


if __name__ == "__main__":
    sys.exit(main())
