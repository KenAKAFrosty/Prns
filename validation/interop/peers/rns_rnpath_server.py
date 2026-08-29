import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

RATE_HASH = bytes.fromhex("33333333333333333333333333333333")


def prepare(
    server_config,
    peer_config,
    bus_port,
    control_port,
    network_port,
    management_identity_path,
):
    server_config = pathlib.Path(server_config)
    peer_config = pathlib.Path(peer_config)
    server_config.mkdir(parents=True, exist_ok=True)
    peer_config.mkdir(parents=True, exist_ok=True)
    management_identity = RNS.Identity()
    management_identity.to_file(management_identity_path)
    server_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        "enable_remote_management = Yes\n"
        f"remote_management_allowed = {management_identity.hash.hex()}\n"
        "publish_blackhole = Yes\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Rnpath Oracle Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {network_port}\n",
        encoding="utf-8",
    )
    peer_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Rnpath Oracle Peer]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {network_port}\n",
        encoding="utf-8",
    )


def serve(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    now = time.time()
    RNS.Transport.announce_rate_table[RATE_HASH] = {
        "last": now - 10,
        "rate_violations": 3,
        "blocked_until": now + 60,
        "timestamps": [now - 3600, now - 10],
    }
    RNS.Transport.remote_management_destination.announce()
    RNS.Transport.blackhole_destination.announce()
    print("RNPATH_SERVER_READY", flush=True)
    while True:
        time.sleep(1)


def peer(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "oracle",
        "rnpath",
    )
    print(f"RNPATH_PEER_READY {destination.hash.hex()}", flush=True)
    while True:
        destination.announce()
        time.sleep(1)


def identity_hash(path):
    identity = RNS.Identity.from_file(path)
    if identity is None:
        raise RuntimeError("identity did not load")
    print(identity.hash.hex())


def main():
    command = sys.argv[1]
    if command == "prepare":
        prepare(*sys.argv[2:])
    elif command == "serve":
        serve(sys.argv[2])
    elif command == "peer":
        peer(sys.argv[2])
    elif command == "identity-hash":
        identity_hash(sys.argv[2])
    else:
        raise RuntimeError(f"unknown command {command}")


if __name__ == "__main__":
    main()
