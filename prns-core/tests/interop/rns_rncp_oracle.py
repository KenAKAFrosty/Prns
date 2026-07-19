import pathlib
import os
import shutil
import sys
import time

import RNS

EXPECTED_RNS_VERSION = "1.3.8"
LISTENER_PRIVATE = bytes([0x31]) * 32 + bytes([0x32]) * 32
CLIENT_PRIVATE = bytes([0x41]) * 32 + bytes([0x42]) * 32


def prepare(
    config_dir,
    client_config_dir,
    bus_port,
    control_port,
    network_port,
    listener_path,
    client_path,
):
    if getattr(RNS, "__version__", "") != EXPECTED_RNS_VERSION:
        raise RuntimeError(f"expected RNS {EXPECTED_RNS_VERSION}")
    config_dir = pathlib.Path(config_dir)
    client_config_dir = pathlib.Path(client_config_dir)
    config_dir.mkdir(parents=True, exist_ok=True)
    client_config_dir.mkdir(parents=True, exist_ok=True)
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[RNCP Network]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {network_port}\n",
        encoding="utf-8",
    )
    client_config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[RNCP Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {network_port}\n",
        encoding="utf-8",
    )
    listener = RNS.Identity.from_bytes(LISTENER_PRIVATE)
    listener.to_file(listener_path)
    RNS.Identity.from_bytes(CLIENT_PRIVATE).to_file(client_path)
    print(RNS.Destination.hash(listener, "rncp", "receive").hex())


def serve(config_dir, listener_path, save_path, fetch_path):
    RNS.Reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    listener = RNS.Identity.from_file(listener_path)
    save_path = pathlib.Path(save_path).resolve()
    fetch_path = pathlib.Path(fetch_path).resolve()
    destination = RNS.Destination(
        listener,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "rncp",
        "receive",
    )

    def concluded(resource):
        if resource.status != RNS.Resource.COMPLETE or resource.metadata is None:
            return
        name = os.path.basename(resource.metadata["name"].decode("utf-8"))
        target = save_path.joinpath(name)
        counter = 0
        while target.exists():
            counter += 1
            target = save_path.joinpath(f"{name}.{counter}")
        shutil.move(resource.data.name, target)

    def established(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_APP)
        link.set_resource_callback(lambda resource: True)
        link.set_resource_concluded_callback(concluded)

    def fetch(path, data, request_id, link_id, remote_identity, requested_at):
        candidate = fetch_path.joinpath(str(data).lstrip("/")).resolve()
        if fetch_path not in candidate.parents or not candidate.is_file():
            return False
        for active in RNS.Transport.active_links:
            if active.link_id == link_id:
                metadata = {"name": candidate.name.encode("utf-8")}
                RNS.Resource(open(candidate, "rb"), active, metadata=metadata)
                return True
        return None

    destination.set_link_established_callback(established)
    destination.register_request_handler(
        "fetch_file",
        response_generator=fetch,
        allow=RNS.Destination.ALLOW_ALL,
    )
    destination.announce()
    print(f"RNCP_SERVER_READY {destination.hash.hex()}", flush=True)
    while True:
        time.sleep(0.25)


def identity_hash(path):
    print(RNS.Identity.from_file(path).hash.hex())


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(*sys.argv[2:])
    elif sys.argv[1] == "serve":
        serve(*sys.argv[2:])
    elif sys.argv[1] == "identity-hash":
        identity_hash(*sys.argv[2:])
    else:
        raise RuntimeError("unknown command")
