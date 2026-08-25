import pathlib
import sys
import time

import RNS
from RNS.vendor import umsgpack
from rns_protocol_evidence import start_reference_reticulum

ENTRY_HASH = bytes.fromhex("33445566778899aabbccddeeff001122")


def seed_publisher(config_dir):
    config_dir = pathlib.Path(config_dir)
    storage = config_dir.joinpath("storage")
    blackhole = storage.joinpath("blackhole")
    blackhole.mkdir(parents=True, exist_ok=True)
    identity = RNS.Identity()
    identity.to_file(storage.joinpath("transport_identity"))
    blackhole.joinpath("local").write_bytes(
        umsgpack.packb(
            {
                ENTRY_HASH: {
                    "source": identity.hash,
                    "until": None,
                    "reason": "interop",
                }
            }
        )
    )
    return identity.hash


def write_server_config(config_dir, port, source_hash):
    pathlib.Path(config_dir).joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = No\n"
        "publish_blackhole = Yes\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Blackhole Publisher]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n",
        encoding="utf-8",
    )
    return source_hash.hex()


def prepare_prns_publisher(server_config, client_config, port):
    source_hash = seed_publisher(server_config)
    write_server_config(server_config, port, source_hash)
    client_config = pathlib.Path(client_config)
    client_config.mkdir(parents=True, exist_ok=True)
    client_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Prns Blackhole Publisher]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n",
        encoding="utf-8",
    )
    print(source_hash.hex())


def prepare_stock_publisher(server_config, client_config, port):
    source_hash = seed_publisher(server_config)
    write_server_config(server_config, port, source_hash)
    client_config = pathlib.Path(client_config)
    client_config.mkdir(parents=True, exist_ok=True)
    client_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        f"blackhole_sources = {source_hash.hex()}\n"
        "blackhole_update_interval = 2\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Stock Blackhole Publisher]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n",
        encoding="utf-8",
    )
    print(source_hash.hex())


def wait_for(predicate, timeout, failure):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError(failure)


def query(config_dir, source_hash):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    source_hash = bytes.fromhex(source_hash)
    destination_hash = RNS.Destination.hash_from_name_and_identity(
        "rnstransport.info.blackhole", source_hash
    )
    if not RNS.Transport.has_path(destination_hash):
        RNS.Transport.request_path(destination_hash)
    wait_for(
        lambda: RNS.Transport.has_path(destination_hash),
        15,
        "path to blackhole publisher was not learned",
    )
    remote_identity = RNS.Identity.recall(destination_hash)
    if remote_identity is None:
        raise RuntimeError("blackhole publisher identity was not recalled")
    destination = RNS.Destination(
        remote_identity,
        RNS.Destination.OUT,
        RNS.Destination.SINGLE,
        "rnstransport",
        "info",
        "blackhole",
    )
    link = RNS.Link(destination)
    wait_for(
        lambda: link.status == RNS.Link.ACTIVE,
        15,
        "link to blackhole publisher did not establish",
    )
    receipt = link.request("/list")
    wait_for(receipt.concluded, 15, "blackhole list request did not conclude")
    response = receipt.get_response()
    link.teardown()
    if not isinstance(response, dict):
        raise RuntimeError(f"expected a blackhole map, got {type(response).__name__}")
    if ENTRY_HASH not in response:
        raise RuntimeError("seeded blackhole identity was absent")
    entry = response[ENTRY_HASH]
    if entry.get("source") != source_hash or entry.get("reason") != "interop":
        raise RuntimeError(f"blackhole entry did not retain its source and reason: {entry!r}")
    print("BLACKHOLE_PUBLISHER_OK")


def serve(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    print("BLACKHOLE_SERVER_READY", flush=True)
    while True:
        time.sleep(1)


def verify_source_file(path, source_hash):
    table = umsgpack.unpackb(pathlib.Path(path).read_bytes())
    source_hash = bytes.fromhex(source_hash)
    if ENTRY_HASH not in table:
        raise RuntimeError("imported source file omitted the seeded identity")
    entry = table[ENTRY_HASH]
    if entry.get("source") != source_hash or entry.get("reason") != "interop":
        raise RuntimeError(f"imported entry changed source metadata: {entry!r}")
    print("BLACKHOLE_UPDATER_OK")


def main():
    command = sys.argv[1]
    commands = {
        "prepare-prns-publisher": prepare_prns_publisher,
        "prepare-stock-publisher": prepare_stock_publisher,
        "query": query,
        "serve": serve,
        "verify-source-file": verify_source_file,
    }
    if command not in commands:
        raise RuntimeError(f"unknown command {command}")
    commands[command](*sys.argv[2:])


if __name__ == "__main__":
    main()
