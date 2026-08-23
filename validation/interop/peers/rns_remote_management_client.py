import pathlib
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum



def prepare(server_config, client_config, port, identity_path):
    server_config = pathlib.Path(server_config)
    client_config = pathlib.Path(client_config)
    server_config.mkdir(parents=True, exist_ok=True)
    client_config.mkdir(parents=True, exist_ok=True)
    identity = RNS.Identity()
    identity.to_file(identity_path)
    identity_hash = identity.hash.hex()
    server_config.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = No\n"
        "enable_remote_management = Yes\n"
        f"remote_management_allowed = {identity_hash}\n"
        "[logging]\n"
        "loglevel = 4\n"
        "[interfaces]\n"
        "[[Remote Test]]\n"
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
        "[[Prns Remote]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {port}\n",
        encoding="utf-8",
    )
    print(identity_hash)


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


def request(link, path, data):
    receipt = link.request(path, data=data)
    wait_for(receipt.concluded, 10, f"{path} request did not conclude")
    response = receipt.get_response()
    if response is None:
        raise RuntimeError(f"{path} request returned no response")
    return response


def rejected_requests(cases):
    receipts = [
        (name, link.request(path, data=data, timeout=2))
        for name, link, path, data in cases
    ]
    time.sleep(3)
    for name, receipt in receipts:
        if receipt.get_status() == RNS.RequestReceipt.READY:
            raise RuntimeError(f"{name} unexpectedly returned {receipt.get_response()!r}")


def query(client_config, transport_hash, identity_path):
    start_reference_reticulum(configdir=client_config, loglevel=RNS.LOG_ERROR)
    transport_identity_hash = bytes.fromhex(transport_hash)
    destination_hash = RNS.Destination.hash_from_name_and_identity(
        "rnstransport.remote.management", transport_identity_hash
    )
    if not RNS.Transport.has_path(destination_hash):
        RNS.Transport.request_path(destination_hash)
    wait_for(
        lambda: RNS.Transport.has_path(destination_hash),
        10,
        "path to remote management destination was not learned",
    )
    remote_identity = RNS.Identity.recall(destination_hash)
    if remote_identity is None:
        raise RuntimeError("remote management identity was not recalled")
    destination = RNS.Destination(
        remote_identity,
        RNS.Destination.OUT,
        RNS.Destination.SINGLE,
        "rnstransport",
        "remote",
        "management",
    )
    unidentified_link = RNS.Link(destination)
    wait_for(
        lambda: unidentified_link.status == RNS.Link.ACTIVE,
        10,
        "unidentified management link did not activate",
    )
    unauthorized_link = RNS.Link(destination)
    wait_for(
        lambda: unauthorized_link.status == RNS.Link.ACTIVE,
        10,
        "unauthorized management link did not activate",
    )
    unauthorized_link.identify(RNS.Identity())
    malformed_link = RNS.Link(destination)
    wait_for(
        lambda: malformed_link.status == RNS.Link.ACTIVE,
        10,
        "malformed-request management link did not activate",
    )
    management_identity = RNS.Identity.from_file(identity_path)
    if management_identity is None:
        raise RuntimeError("management identity did not load")
    malformed_link.identify(management_identity)
    rejected_requests(
        [
            ("unidentified status", unidentified_link, "/status", [True]),
            ("unauthorized status", unauthorized_link, "/status", [True]),
            ("malformed status", malformed_link, "/status", []),
            ("malformed path", malformed_link, "/path", []),
            ("malformed rates", malformed_link, "/path", [123]),
        ]
    )
    unidentified_link.teardown()
    unauthorized_link.teardown()
    malformed_link.teardown()
    link = RNS.Link(destination)
    wait_for(lambda: link.status == RNS.Link.ACTIVE, 10, "management link did not recover")
    link.identify(management_identity)
    status = request(link, "/status", [True])
    if not isinstance(status, list) or len(status) != 2:
        raise RuntimeError(f"unexpected status response: {status!r}")
    if not isinstance(status[0], dict) or "interfaces" not in status[0]:
        raise RuntimeError(f"status body is not an interface report: {status!r}")
    if not isinstance(status[1], int):
        raise RuntimeError(f"link count is not an integer: {status!r}")
    table = request(link, "/path", ["table", None, None])
    if not isinstance(table, list):
        raise RuntimeError(f"path table is not a list: {table!r}")
    rates = request(link, "/path", ["rates", None])
    if not isinstance(rates, list):
        raise RuntimeError(f"rate table is not a list: {rates!r}")
    print(
        f"REMOTE_MANAGEMENT_OK interfaces={len(status[0]['interfaces'])} "
        f"links={status[1]} paths={len(table)} rates={len(rates)} hostile_cases=5"
    )


def main():
    command = sys.argv[1]
    if command == "prepare":
        prepare(*sys.argv[2:])
    elif command == "identity-hash":
        identity_hash(sys.argv[2])
    elif command == "query":
        query(*sys.argv[2:])
    else:
        raise RuntimeError(f"unknown command {command}")


if __name__ == "__main__":
    main()
