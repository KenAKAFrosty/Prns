#!/usr/bin/env python3
"""Stock-RNS control-RPC oracle for a Prns shared instance.

This intentionally drives Reticulum's public methods rather than hand-crafting
frames. The manifest-pinned stock RNS lane uses MessagePack; an explicit compatibility
lane can select the older pickle payload while exercising the same methods.
"""

import os
import hashlib
import hmac
import multiprocessing.connection
import socket
import struct
import sys
import tempfile
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum
from RNS.vendor import umsgpack as mp

LEGACY_PICKLE = os.environ.get("RPC_SMOKE_LEGACY_PICKLE") == "1"
RPC_FRAME_MAX_LENGTH = 16_777_216
EXPECTED_RPC_SURFACE = frozenset(
    {
        "blackhole_identity",
        "blackholed_identities",
        "destination_data_retain",
        "destination_data_unretain",
        "destination_data_used",
        "drop_all_via",
        "drop_announce_queues",
        "drop_path",
        "first_hop_timeout",
        "identity_data_retain",
        "interface_stats",
        "is_blackholed",
        "link_count",
        "next_hop",
        "next_hop_if_name",
        "packet_q",
        "packet_rssi",
        "packet_snr",
        "path_table",
        "rate_table",
        "unblackhole_identity",
    }
)


def fail(message):
    print("RPC_ORACLE_FAIL " + message, file=sys.stderr)
    return 1


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def record(covered, operation, result):
    covered.add(operation)
    return result


def recv_exact(peer, length):
    received = bytearray()
    while len(received) < length:
        chunk = peer.recv(length - len(received))
        if not chunk:
            raise EOFError(f"connection closed with {length - len(received)} bytes pending")
        received.extend(chunk)
    return bytes(received)


def recv_frame(peer):
    short = struct.unpack("!i", recv_exact(peer, 4))[0]
    if short == -1:
        length = struct.unpack("!Q", recv_exact(peer, 8))[0]
    elif short < 0:
        raise ValueError(f"negative frame length {short}")
    else:
        length = short
    return recv_exact(peer, length)


def send_frame(peer, payload):
    peer.sendall(struct.pack("!i", len(payload)) + payload)


def authenticate_raw(peer, key):
    challenge = recv_frame(peer)
    require(challenge.startswith(b"#CHALLENGE#"), "server challenge prefix")
    message = challenge[len(b"#CHALLENGE#") :]
    require(message.startswith(b"{sha256}"), "server challenge digest")
    send_frame(peer, b"{sha256}" + hmac.new(key, message, hashlib.sha256).digest())
    require(recv_frame(peer) == b"#WELCOME#", "server did not welcome valid MAC")
    our_message = b"{sha256}" + bytes(range(20))
    send_frame(peer, b"#CHALLENGE#" + our_message)
    response = recv_frame(peer)
    require(response.startswith(b"{sha256}"), "server response digest")
    require(
        hmac.compare_digest(
            response[len(b"{sha256}") :], hmac.new(key, our_message, hashlib.sha256).digest()
        ),
        "server authentication MAC",
    )
    send_frame(peer, b"#WELCOME#")


def raw_peer(port):
    peer = socket.create_connection(("127.0.0.1", port), timeout=3.0)
    peer.settimeout(3.0)
    return peer


def prove_recovery(port, key):
    connection = multiprocessing.connection.Client(("127.0.0.1", port), authkey=key)
    try:
        connection.send_bytes(mp.packb({"get": "link_count"}))
        require(isinstance(mp.unpackb(connection.recv_bytes()), int), "recovery link_count")
    finally:
        connection.close()


def hostile_preflight(port, key):
    peer = raw_peer(port)
    challenge = recv_frame(peer)
    message = challenge[len(b"#CHALLENGE#") :]
    send_frame(peer, b"{sha256}" + hmac.new(bytes([0x6B]) * 32, message, hashlib.sha256).digest())
    require(recv_frame(peer) == b"#FAILURE#", "wrong key was not rejected")
    peer.close()
    prove_recovery(port, key)

    cases = [
        ("negative", struct.pack("!i", -2)),
        ("oversized", struct.pack("!i", RPC_FRAME_MAX_LENGTH + 1)),
        ("truncated", struct.pack("!i", 8) + b"\x81\xa3"),
        ("malformed", struct.pack("!i", 1) + b"\xc1"),
        (
            "unknown",
            struct.pack("!i", len(mp.packb({"get": "future"})))
            + mp.packb({"get": "future"}),
        ),
        ("half-closed", b""),
    ]
    for name, payload in cases:
        peer = raw_peer(port)
        authenticate_raw(peer, key)
        if payload:
            peer.sendall(payload)
        peer.shutdown(socket.SHUT_WR)
        peer.close()
        prove_recovery(port, key)
    return len(cases) + 1


def main() -> int:
    local_port = int(os.environ["PRNS_LOCAL_PORT"])
    rpc_port = int(os.environ["PRNS_RPC_PORT"])
    rpc_key = os.environ.get("PRNS_RPC_KEY", "5a" * 32)
    rpc_key_bytes = bytes.fromhex(rpc_key)

    configdir = tempfile.mkdtemp(prefix="rns-rpc-oracle-")
    config = f"""[reticulum]
  enable_transport = No
  share_instance = Yes
  shared_instance_type = tcp
  shared_instance_port = {local_port}
  instance_control_port = {rpc_port}
  rpc_key = {rpc_key}
  panic_on_interface_error = No

[logging]
  loglevel = 3
"""
    with open(os.path.join(configdir, "config"), "w", encoding="utf-8") as handle:
        handle.write(config)

    hostile_cases = 0 if LEGACY_PICKLE else hostile_preflight(rpc_port, rpc_key_bytes)
    reticulum = start_reference_reticulum(
        configdir=configdir,
        loglevel=RNS.LOG_WARNING,
    )
    time.sleep(1.0)
    covered = set()

    try:
        stats = record(covered, "interface_stats", reticulum.get_interface_stats())
        require(isinstance(stats, dict), "interface_stats is not a dict")
        for key in ("interfaces", "rxb", "txb", "rxs", "txs", "rss"):
            require(key in stats, f"interface_stats missing {key}")
        require(isinstance(stats["interfaces"], list), "interfaces is not a list")
        for row in stats["interfaces"]:
            require(isinstance(row, dict), "interface row is not a dict")
            for key in ("name", "short_name", "type", "status", "mode", "rxb", "txb"):
                require(key in row, f"interface row missing {key}")

        link_count = record(covered, "link_count", reticulum.get_link_count())
        require(isinstance(link_count, int), "link_count is not an int")

        path_table = record(covered, "path_table", reticulum.get_path_table(max_hops=8))
        require(isinstance(path_table, list), "path_table is not a list")

        rate_table = record(covered, "rate_table", reticulum.get_rate_table())
        require(isinstance(rate_table, list), "rate_table is not a list")

        blackholed = record(
            covered, "blackholed_identities", reticulum.get_blackholed_identities()
        )
        require(isinstance(blackholed, dict), "blackholed_identities is not a dict")

        known_identity = RNS.Identity()
        known_destination = RNS.Destination(
            known_identity,
            RNS.Destination.IN,
            RNS.Destination.SINGLE,
            "prns",
            "rpc_oracle",
        )
        known_destination.announce(app_data=b"rpc-oracle")
        known_next_hop = None
        announce_deadline = time.time() + 5.0
        while known_next_hop is None and time.time() < announce_deadline:
            time.sleep(0.1)
            known_next_hop = reticulum.get_next_hop(known_destination.hash)
        require(known_next_hop is not None, "announced destination did not reach Prns")

        require(
            record(
                covered,
                "destination_data_used",
                reticulum._used_destination_data(known_destination.hash),
            )
            is True,
            "known destination use was not recorded",
        )
        require(
            record(
                covered,
                "destination_data_retain",
                reticulum._retain_destination_data(known_destination.hash),
            )
            is True,
            "known destination was not retained",
        )
        require(
            reticulum._used_destination_data(known_destination.hash) is False,
            "retained destination incorrectly recorded use",
        )
        require(
            record(
                covered,
                "destination_data_unretain",
                reticulum._unretain_destination_data(known_destination.hash),
            )
            is True,
            "known destination was not unretained",
        )
        require(
            reticulum._used_destination_data(known_destination.hash) is True,
            "unretained destination did not record use",
        )
        require(
            record(
                covered,
                "identity_data_retain",
                reticulum._retain_identity(known_identity.hash),
            )
            is True,
            "known identity was not retained",
        )
        require(
            reticulum._retain_destination_data(known_destination.hash) is True,
            "already retained destination did not report success",
        )
        require(
            reticulum._used_destination_data(known_destination.hash) is False,
            "identity-retained destination incorrectly recorded use",
        )

        unknown_destination = bytes([0x11] * 16)
        next_hop = record(covered, "next_hop", reticulum.get_next_hop(unknown_destination))
        require(next_hop is None, "unknown next_hop is not None")
        next_hop_if_name = record(
            covered,
            "next_hop_if_name",
            reticulum.get_next_hop_if_name(unknown_destination),
        )
        require(
            next_hop_if_name == "None",
            "unknown next_hop_if_name is not 'None'",
        )
        first_hop_timeout = record(
            covered,
            "first_hop_timeout",
            reticulum.get_first_hop_timeout(unknown_destination),
        )
        require(
            first_hop_timeout == 6,
            "first_hop_timeout is not the RNS default",
        )
        drop_path = record(covered, "drop_path", reticulum.drop_path(unknown_destination))
        require(drop_path is False, "unknown drop_path is not False")
        drop_all_via = record(
            covered, "drop_all_via", reticulum.drop_all_via(unknown_destination)
        )
        require(drop_all_via == 0, "drop_all_via did not report zero drops")
        drop_announce_queues = record(
            covered, "drop_announce_queues", reticulum.drop_announce_queues()
        )
        require(drop_announce_queues is None, "drop_announce_queues is not None")

        packet_hash = bytes([0x22] * 16)
        packet_rssi = record(covered, "packet_rssi", reticulum.get_packet_rssi(packet_hash))
        packet_snr = record(covered, "packet_snr", reticulum.get_packet_snr(packet_hash))
        packet_q = record(covered, "packet_q", reticulum.get_packet_q(packet_hash))
        require(packet_rssi is None, "packet_rssi is not None")
        require(packet_snr is None, "packet_snr is not None")
        require(packet_q is None, "packet_q is not None")

        identity_hash = bytes([0x33] * 16)
        is_blackholed = record(
            covered, "is_blackholed", reticulum.is_blackholed(identity_hash)
        )
        require(is_blackholed is False, "unknown identity is blackholed")
        blackhole_identity = record(
            covered, "blackhole_identity", reticulum.blackhole_identity(identity_hash)
        )
        require(
            blackhole_identity is True,
            "blackhole_identity did not add the identity",
        )
        require(reticulum.is_blackholed(identity_hash) is True, "identity was not blackholed")
        require(
            identity_hash in reticulum.get_blackholed_identities(),
            "blackholed identity was absent from the table",
        )
        unblackhole_identity = record(
            covered, "unblackhole_identity", reticulum.unblackhole_identity(identity_hash)
        )
        require(
            unblackhole_identity is True,
            "unblackhole_identity did not remove the identity",
        )
        require(reticulum.is_blackholed(identity_hash) is False, "identity stayed blackholed")
        require(
            reticulum.unblackhole_identity(identity_hash) is None,
            "missing unblackhole did not return None",
        )
        destination_data_used = reticulum._used_destination_data(unknown_destination)
        require(
            destination_data_used is False,
            "destination_data used unexpectedly succeeded",
        )
        destination_data_retain = reticulum._retain_destination_data(unknown_destination)
        require(
            destination_data_retain is False,
            "destination_data retain unexpectedly succeeded",
        )
        destination_data_unretain = reticulum._unretain_destination_data(unknown_destination)
        require(
            destination_data_unretain is False,
            "destination_data unretain unexpectedly succeeded",
        )
        identity_data_retain = reticulum._retain_identity(identity_hash)
        require(
            identity_data_retain is False,
            "identity_data retain unexpectedly succeeded",
        )
        require(
            covered == EXPECTED_RPC_SURFACE,
            "RPC surface coverage mismatch: "
            f"missing={sorted(EXPECTED_RPC_SURFACE - covered)!r} "
            f"unexpected={sorted(covered - EXPECTED_RPC_SURFACE)!r}",
        )
    except Exception as error:
        return fail(str(error))

    print(
        "RPC_ORACLE_OK "
        f"interfaces={len(stats['interfaces'])} "
        f"links={link_count} "
        f"paths={len(path_table)} "
        f"rates={len(rate_table)} "
        f"blackholes={len(blackholed)} "
        f"operations={len(covered)} "
        f"hostile_cases={hostile_cases}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
