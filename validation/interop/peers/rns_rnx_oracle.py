import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

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
        "[[RNX Network]]\n"
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
        "[[RNX Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {network_port}\n",
        encoding="utf-8",
    )
    listener = RNS.Identity.from_bytes(LISTENER_PRIVATE)
    listener.to_file(listener_path)
    RNS.Identity.from_bytes(CLIENT_PRIVATE).to_file(client_path)
    print(RNS.Destination.hash(listener, "rnx", "execute").hex())


def serve(config_dir, listener_path):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    listener = RNS.Identity.from_file(listener_path)
    destination = RNS.Destination(
        listener,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "rnx",
        "execute",
    )

    def execute(path, data, request_id, link_id, remote_identity, requested_at):
        command, timeout, stdout_limit, stderr_limit, stdin = data
        command = command.decode("utf-8")
        identity_state = b"identified" if remote_identity is not None else b"anonymous"
        stdout = identity_state + b":" + command.encode("utf-8")
        if stdin is not None:
            stdout += b":" + stdin
        stderr = b"stock-stderr"
        total_stdout = len(stdout)
        total_stderr = len(stderr)
        if stdout_limit is not None:
            stdout = stdout[:stdout_limit]
        if stderr_limit is not None:
            stderr = stderr[:stderr_limit]
        started = time.time()
        return [
            True,
            7,
            stdout,
            stderr,
            total_stdout,
            total_stderr,
            started,
            time.time(),
        ]

    destination.register_request_handler(
        "command",
        response_generator=execute,
        allow=RNS.Destination.ALLOW_ALL,
    )
    destination.announce()
    print(f"RNX_SERVER_READY {destination.hash.hex()}", flush=True)
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
