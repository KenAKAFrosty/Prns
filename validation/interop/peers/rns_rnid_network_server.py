import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

LOOKUP_PRIVATE = bytes([0x33]) * 32 + bytes([0x44]) * 32
ANNOUNCE_PRIVATE = bytes([0x55]) * 32 + bytes([0x66]) * 32


def prepare(config_dir, bus_port, control_port, announce_identity_path):
    config_dir = pathlib.Path(config_dir)
    config_dir.mkdir(parents=True, exist_ok=True)
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n",
        encoding="utf-8",
    )
    RNS.Identity.from_bytes(ANNOUNCE_PRIVATE).to_file(announce_identity_path)


class AnnounceObserver:
    aspect_filter = "oracle.identity"

    def __init__(self, expected_identity_hash):
        self.expected_identity_hash = expected_identity_hash

    def received_announce(self, destination_hash, announced_identity, app_data):
        if announced_identity.hash == self.expected_identity_hash:
            print(
                f"RNID_ANNOUNCE_RECEIVED {destination_hash.hex()} {announced_identity.hash.hex()}",
                flush=True,
            )


def serve(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    lookup_identity = RNS.Identity.from_bytes(LOOKUP_PRIVATE)
    lookup_destination = RNS.Destination(
        lookup_identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "rns",
        "id",
    )
    announce_identity = RNS.Identity.from_bytes(ANNOUNCE_PRIVATE)
    RNS.Transport.register_announce_handler(AnnounceObserver(announce_identity.hash))
    lookup_destination.announce()
    print(
        f"RNID_SERVER_READY {lookup_identity.hash.hex()} {lookup_destination.hash.hex()} {announce_identity.hash.hex()}",
        flush=True,
    )
    last_announce = time.time()
    while True:
        time.sleep(0.05)
        if time.time() - last_announce >= 0.25:
            lookup_destination.announce()
            last_announce = time.time()


def main():
    command = sys.argv[1]
    if command == "prepare":
        prepare(*sys.argv[2:])
    elif command == "serve":
        serve(sys.argv[2])
    else:
        raise RuntimeError(f"unknown command {command}")


if __name__ == "__main__":
    main()
