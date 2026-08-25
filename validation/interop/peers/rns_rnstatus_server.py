import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum



def prepare(config_dir, bus_port, control_port, management_identity_path):
    config_dir = pathlib.Path(config_dir)
    config_dir.mkdir(parents=True, exist_ok=True)
    management_identity = RNS.Identity()
    management_identity.to_file(management_identity_path)
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        "enable_remote_management = Yes\n"
        f"remote_management_allowed = {management_identity.hash.hex()}\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n",
        encoding="utf-8",
    )


def serve(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    print("RNSTATUS_SERVER_READY", flush=True)
    while True:
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
    elif command == "identity-hash":
        identity_hash(sys.argv[2])
    else:
        raise RuntimeError(f"unknown command {command}")


if __name__ == "__main__":
    main()
