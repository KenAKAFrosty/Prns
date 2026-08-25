import os
import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum



def prepare(server_config, client_config, port):
    server_config = pathlib.Path(server_config)
    client_config = pathlib.Path(client_config)
    server_config.mkdir(parents=True, exist_ok=True)
    client_config.mkdir(parents=True, exist_ok=True)
    server_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = No\n"
        "respond_to_probes = Yes\n"
        "[logging]\n"
        "loglevel = 4\n"
        "[interfaces]\n"
        "[[Probe Test]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n",
        encoding="utf-8",
    )
    client_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Prns Probe]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n",
        encoding="utf-8",
    )


def identity_hash(path):
    identity = RNS.Identity.from_file(path)
    if identity is None:
        raise RuntimeError("transport identity did not load")
    print(identity.hash.hex())


def wait_for(predicate, timeout, failure):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError(failure)


def probe(client_config, transport_hash):
    start_reference_reticulum(configdir=client_config, loglevel=RNS.LOG_ERROR)
    transport_identity_hash = bytes.fromhex(transport_hash)
    destination_hash = RNS.Destination.hash_from_name_and_identity(
        "rnstransport.probe", transport_identity_hash
    )
    if not RNS.Transport.has_path(destination_hash):
        RNS.Transport.request_path(destination_hash)
    wait_for(
        lambda: RNS.Transport.has_path(destination_hash),
        10,
        "path to probe destination was not learned",
    )
    remote_identity = RNS.Identity.recall(destination_hash)
    if remote_identity is None:
        raise RuntimeError("probe identity was not recalled")
    destination = RNS.Destination(
        remote_identity,
        RNS.Destination.OUT,
        RNS.Destination.SINGLE,
        "rnstransport",
        "probe",
    )
    receipt = RNS.Packet(destination, os.urandom(16)).send()
    wait_for(
        lambda: receipt.get_status() != RNS.PacketReceipt.SENT,
        10,
        "probe packet did not conclude",
    )
    if receipt.get_status() != RNS.PacketReceipt.DELIVERED:
        raise RuntimeError(f"probe packet failed with status {receipt.get_status()}")
    print(f"PROBE_RESPONDER_OK rtt={receipt.get_rtt():.6f}")


def main():
    command = sys.argv[1]
    if command == "prepare":
        prepare(*sys.argv[2:])
    elif command == "identity-hash":
        identity_hash(sys.argv[2])
    elif command == "probe":
        probe(*sys.argv[2:])
    else:
        raise RuntimeError(f"unknown command {command}")


if __name__ == "__main__":
    main()
