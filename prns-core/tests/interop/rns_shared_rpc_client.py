#!/usr/bin/env python3
"""RNS 1.3.8 msgpack control-RPC oracle for a Prns shared instance.

This intentionally drives Reticulum's public methods rather than hand-crafting
frames. In RNS 1.3.8 those methods send msgpack RPC payloads over
``multiprocessing.connection`` with ``send_bytes(mp.packb(...))`` and decode
replies with ``mp.unpackb(recv_bytes())``.
"""

import os
import sys
import tempfile
import time

import RNS

EXPECTED_RNS_VERSION = "1.3.8"
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


def main() -> int:
    version = getattr(RNS, "__version__", "")
    if version != EXPECTED_RNS_VERSION:
        return fail(f"expected RNS {EXPECTED_RNS_VERSION}, got {version!r}")

    local_port = int(os.environ["PRNS_LOCAL_PORT"])
    rpc_port = int(os.environ["PRNS_RPC_PORT"])
    rpc_key = os.environ.get("PRNS_RPC_KEY", "5a" * 32)

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

    reticulum = RNS.Reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)
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
            blackhole_identity is False,
            "blackhole_identity unexpectedly succeeded",
        )
        unblackhole_identity = record(
            covered, "unblackhole_identity", reticulum.unblackhole_identity(identity_hash)
        )
        require(
            unblackhole_identity is False,
            "unblackhole_identity unexpectedly succeeded",
        )
        destination_data_used = record(
            covered,
            "destination_data_used",
            reticulum._used_destination_data(unknown_destination),
        )
        require(
            destination_data_used is False,
            "destination_data used unexpectedly succeeded",
        )
        destination_data_retain = record(
            covered,
            "destination_data_retain",
            reticulum._retain_destination_data(unknown_destination),
        )
        require(
            destination_data_retain is False,
            "destination_data retain unexpectedly succeeded",
        )
        destination_data_unretain = record(
            covered,
            "destination_data_unretain",
            reticulum._unretain_destination_data(unknown_destination),
        )
        require(
            destination_data_unretain is False,
            "destination_data unretain unexpectedly succeeded",
        )
        identity_data_retain = record(
            covered, "identity_data_retain", reticulum._retain_identity(identity_hash)
        )
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
        f"operations={len(covered)}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
