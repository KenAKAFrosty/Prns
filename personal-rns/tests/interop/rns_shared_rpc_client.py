#!/usr/bin/env python3
"""RNS 1.3.5 msgpack control-RPC oracle for a Prns shared instance.

This intentionally drives Reticulum's public methods rather than hand-crafting
frames. In RNS 1.3.5 those methods send msgpack RPC payloads over
``multiprocessing.connection`` with ``send_bytes(mp.packb(...))`` and decode
replies with ``mp.unpackb(recv_bytes())``.
"""

import os
import sys
import tempfile
import time

import RNS


def fail(message):
    print("RPC_ORACLE_FAIL " + message, file=sys.stderr)
    return 1


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def main() -> int:
    version = getattr(RNS, "__version__", "")
    if version != "1.3.5":
        return fail(f"expected RNS 1.3.5, got {version!r}")

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

    try:
        stats = reticulum.get_interface_stats()
        require(isinstance(stats, dict), "interface_stats is not a dict")
        for key in ("interfaces", "rxb", "txb", "rxs", "txs", "rss"):
            require(key in stats, f"interface_stats missing {key}")
        require(isinstance(stats["interfaces"], list), "interfaces is not a list")
        for row in stats["interfaces"]:
            require(isinstance(row, dict), "interface row is not a dict")
            for key in ("name", "short_name", "type", "status", "mode", "rxb", "txb"):
                require(key in row, f"interface row missing {key}")

        link_count = reticulum.get_link_count()
        require(isinstance(link_count, int), "link_count is not an int")

        path_table = reticulum.get_path_table(max_hops=8)
        require(isinstance(path_table, list), "path_table is not a list")

        unknown_destination = bytes([0x11] * 16)
        require(reticulum.get_next_hop(unknown_destination) is None, "unknown next_hop is not None")
        require(
            reticulum.get_next_hop_if_name(unknown_destination) == "None",
            "unknown next_hop_if_name is not 'None'",
        )
        require(
            reticulum.get_first_hop_timeout(unknown_destination) == 6,
            "first_hop_timeout is not the RNS default",
        )

        packet_hash = bytes([0x22] * 16)
        require(reticulum.get_packet_rssi(packet_hash) is None, "packet_rssi is not None")
        require(reticulum.get_packet_snr(packet_hash) is None, "packet_snr is not None")
        require(reticulum.get_packet_q(packet_hash) is None, "packet_q is not None")
    except Exception as error:
        return fail(str(error))

    print(
        "RPC_ORACLE_OK "
        f"interfaces={len(stats['interfaces'])} "
        f"links={link_count} "
        f"paths={len(path_table)}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
