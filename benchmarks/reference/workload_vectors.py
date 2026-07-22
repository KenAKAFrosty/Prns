#!/usr/bin/env python3
"""Deterministic workload vectors shared by the compiled-reference participant.

This module deliberately has no RNS or third-party imports, so qualification can
prove the payload and size law before starting either implementation.
"""

import json
import sys
from pathlib import Path

DEFAULT_SIZE_SEED = 0x5EEDCAFEF00D0001
MASK64 = 0xFFFFFFFFFFFFFFFF


class SizeSequence:
    def __init__(self, seed, lo, hi, fixed):
        if not hi:
            lo, hi = fixed, fixed
        self.state = seed & MASK64
        self.lo = lo
        self.hi = hi

    def next_len(self):
        return self.next_in(self.lo, self.hi)

    def next_in(self, lo, hi):
        state = self.state
        state = (state ^ (state << 13)) & MASK64
        state = (state ^ (state >> 7)) & MASK64
        state = (state ^ (state << 17)) & MASK64
        self.state = state
        return lo + (state % (hi - lo + 1))


def deterministic_payload(length):
    state = 0x9E3779B97F4A7C15
    data = bytearray()
    while len(data) < length:
        state ^= (state << 13) & MASK64
        state ^= state >> 7
        state ^= (state << 17) & MASK64
        state &= MASK64
        data.extend(state.to_bytes(8, "little"))
    return bytes(data[:length])


def verify_golden(path):
    golden = json.loads(Path(path).read_text(encoding="utf-8"))
    sizes = SizeSequence(
        golden["seed"], golden["size_min"], golden["size_max"], golden["size_min"]
    )
    actual_sizes = [sizes.next_len() for _ in golden["sizes"]]
    actual_payload = deterministic_payload(golden["payload_len"]).hex()
    if actual_sizes != golden["sizes"] or actual_payload != golden["payload_hex"]:
        raise SystemExit(
            f"deterministic workload drift: sizes={actual_sizes}, payload={actual_payload}"
        )
    print("WORKLOAD_VECTORS verified Rust/Python golden sizes and payload", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: workload_vectors.py <workload-vectors.json>")
    verify_golden(sys.argv[1])
