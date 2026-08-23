import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum



def prepare(config_dir, bus_port, control_port):
    config_dir = pathlib.Path(config_dir)
    config_dir.mkdir(parents=True, exist_ok=True)
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        "respond_to_probes = Yes\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n",
        encoding="utf-8",
    )


def serve(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    silent_identity = RNS.Identity()
    silent = RNS.Destination(
        silent_identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "oracle",
        "silent",
    )
    silent.set_proof_strategy(RNS.Destination.PROVE_NONE)
    probe = RNS.Transport.probe_destination
    probe.announce()
    silent.announce()
    print(
        f"RNPROBE_SERVER_READY {probe.hash.hex()} {silent.hash.hex()}",
        flush=True,
    )
    last_announce = time.time()
    while True:
        time.sleep(0.1)
        if time.time() - last_announce >= 1:
            probe.announce()
            silent.announce()
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
