#!/usr/bin/env python3

import json
import multiprocessing.connection
import os
import struct
import sys

import RNS
import RNS.vendor.umsgpack as msgpack


RPC_FRAME_MAX_LENGTH = 16_777_216


class CaptureConnection:
    def __init__(self):
        self.payloads = []

    def send_bytes(self, payload):
        self.payloads.append(bytes(payload))

    def recv_bytes(self):
        return msgpack.packb(None)


class VirtualPayload:
    def __init__(self, length):
        self.length = length

    def __len__(self):
        return self.length


class HeaderCapture:
    def __init__(self):
        self.parts = []

    def _send(self, value):
        if isinstance(value, bytes):
            self.parts.append(value)


def capture_header(length):
    sender = HeaderCapture()
    payload = b"" if length == 0 else VirtualPayload(length)
    multiprocessing.connection.Connection._send_bytes(sender, payload)
    return b"".join(sender.parts).hex()


def capture_requests():
    connection = CaptureConnection()
    reticulum = object.__new__(RNS.Reticulum)
    reticulum.is_connected_to_shared_instance = True
    reticulum.get_rpc_client = lambda: connection
    destination_hash = bytes([0x11] * 16)
    packet_hash = bytes([0x22] * 32)
    identity_hash = bytes([0x33] * 16)
    calls = [
        ("interface_stats", lambda: reticulum.get_interface_stats()),
        ("path_table", lambda: reticulum.get_path_table(max_hops=8)),
        ("rate_table", lambda: reticulum.get_rate_table()),
        ("next_hop_if_name", lambda: reticulum.get_next_hop_if_name(destination_hash)),
        ("next_hop", lambda: reticulum.get_next_hop(destination_hash)),
        ("first_hop_timeout", lambda: reticulum.get_first_hop_timeout(destination_hash)),
        ("link_count", lambda: reticulum.get_link_count()),
        ("packet_rssi", lambda: reticulum.get_packet_rssi(packet_hash)),
        ("packet_snr", lambda: reticulum.get_packet_snr(packet_hash)),
        ("packet_q", lambda: reticulum.get_packet_q(packet_hash)),
        ("blackholed_identities", lambda: reticulum.get_blackholed_identities()),
        ("is_blackholed", lambda: reticulum.is_blackholed(identity_hash)),
        ("drop_path", lambda: reticulum.drop_path(destination_hash)),
        ("drop_all_via", lambda: reticulum.drop_all_via(destination_hash)),
        ("drop_announce_queues", lambda: reticulum.drop_announce_queues()),
        (
            "blackhole_identity",
            lambda: reticulum.blackhole_identity(
                identity_hash, until=2_147_483_648, reason="oracle"
            ),
        ),
        ("unblackhole_identity", lambda: reticulum.unblackhole_identity(identity_hash)),
        ("destination_data_used", lambda: reticulum._used_destination_data(destination_hash)),
        (
            "destination_data_retain",
            lambda: reticulum._retain_destination_data(destination_hash),
        ),
        (
            "destination_data_unretain",
            lambda: reticulum._unretain_destination_data(destination_hash),
        ),
        ("identity_data_retain", lambda: reticulum._retain_identity(identity_hash)),
    ]
    captured = []
    for name, call in calls:
        before = len(connection.payloads)
        call()
        if len(connection.payloads) != before + 1:
            raise RuntimeError(f"{name} emitted {len(connection.payloads) - before} payloads")
        captured.append({"name": name, "hex": connection.payloads[-1].hex()})
    return captured


def packed_pairs(pairs):
    if len(pairs) >= 16:
        raise ValueError("the oracle's duplicate-field helper only needs fixmap")
    return bytes([0x80 | len(pairs)]) + b"".join(
        msgpack.packb(key, use_bin_type=True) + msgpack.packb(value, use_bin_type=True)
        for key, value in pairs
    )


def mutation_corpus(canonical):
    base = bytes.fromhex(canonical[4]["hex"])
    mutations = [
        ("empty", b""),
        ("non-map", msgpack.packb(None, use_bin_type=True)),
        ("missing-operation", msgpack.packb({"destination_hash": b"x" * 16}, use_bin_type=True)),
        ("missing-required-field", msgpack.packb({"get": "next_hop"}, use_bin_type=True)),
        (
            "extra-field",
            msgpack.packb({"get": "link_count", "reason": "extra"}, use_bin_type=True),
        ),
        ("wrong-operation-type", msgpack.packb({"get": 1}, use_bin_type=True)),
        (
            "wrong-scalar-type",
            msgpack.packb({"get": "path_table", "max_hops": "8"}, use_bin_type=True),
        ),
        ("unknown-operation", msgpack.packb({"get": "future"}, use_bin_type=True)),
        ("unknown-field", msgpack.packb({"future": "link_count"}, use_bin_type=True)),
        (
            "contradictory-operation",
            msgpack.packb({"get": "link_count", "drop": "announce_queues"}, use_bin_type=True),
        ),
        (
            "duplicate-field",
            packed_pairs([("get", "link_count"), ("get", "rate_table")]),
        ),
        ("trailing-bytes", base + msgpack.packb(None, use_bin_type=True)),
        (
            "integer-overflow-extension",
            msgpack.packb(
                {
                    "get": "path_table",
                    "max_hops": msgpack.Ext(1, (1 << 64).to_bytes(9, "big")),
                },
                use_bin_type=True,
            ),
        ),
        ("declared-map-overflow", bytes.fromhex("dfffffffff")),
        ("declared-binary-overflow", bytes.fromhex("81a6726561736f6ec6ffffffff")),
    ]
    for index in range(len(base)):
        mutations.append((f"truncated-{index}", base[:index]))
    return [{"name": name, "hex": payload.hex()} for name, payload in mutations]


def integer_boundaries():
    values = [-(1 << 63), -(1 << 31), -1, 0, (1 << 31) - 1, 1 << 31, (1 << 63) - 1, (1 << 64) - 1]
    return [
        {
            "value": str(value),
            "hex": msgpack.packb(
                {"get": "path_table", "max_hops": value}, use_bin_type=True
            ).hex(),
        }
        for value in values
    ]


def main():
    canonical = capture_requests()
    headers = [
        {"length": str(length), "hex": capture_header(length)}
        for length in [
            0,
            (1 << 31) - 1,
            1 << 31,
            RPC_FRAME_MAX_LENGTH,
            RPC_FRAME_MAX_LENGTH + 1,
        ]
    ]
    json.dump(
        {
            "version": RNS.__version__,
            "canonical": canonical,
            "mutations": mutation_corpus(canonical),
            "integer_boundaries": integer_boundaries(),
            "headers": headers,
            "request_frame_max_length": RPC_FRAME_MAX_LENGTH,
        },
        sys.stdout,
        sort_keys=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
