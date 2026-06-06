"""RetiNet's announce-parallel harness (an RNS fork; ships the `RNS` module). Shard the
corpus across Python threads, each Packet.unpack + Identity.validate_announce, best-of-N min
wall. Conformance is the resolved count from a single pass. Swept single-thread vs
os.cpu_count(); prints the parallel RESULT line for run-mt.sh.

PyCA's Ed25519 verify holds the GIL, so this is the honest record of what threads buy a
pure-Python runtime here (≈1×) — not a bug in the harness."""

import os
import sys
import threading
import time

import RNS
import RNS.Transport

WARMUP = 5
ITERS = 30


def decode(path):
    return [bytes.fromhex(line) for line in open(path).read().split()]


def ingest(chunk):
    for raw in chunk:
        packet = RNS.Packet(None, raw)
        packet.unpack()
        RNS.Identity.validate_announce(packet)


def conformance(raws):
    RNS.Identity.known_destinations = {}
    resolved = 0
    for raw in raws:
        packet = RNS.Packet(None, raw)
        packet.unpack()
        if RNS.Identity.validate_announce(packet):
            resolved += 1
    return resolved


def throughput_at(raws, t):
    total = len(raws)
    chunks = [raws[i::t] for i in range(t)]
    best = float("inf")
    for i in range(WARMUP + ITERS):
        RNS.Identity.known_destinations = {}
        start = time.perf_counter()
        workers = [threading.Thread(target=ingest, args=(c,)) for c in chunks]
        for w in workers:
            w.start()
        for w in workers:
            w.join()
        secs = time.perf_counter() - start
        if i >= WARMUP:
            best = min(best, secs)
    return total / best


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: driver-mt.py <corpus.hex>")
    raws = decode(sys.argv[1])

    resolved = conformance(raws)
    lo, hi = 1, os.cpu_count() or 1
    lo_ps = throughput_at(raws, lo)
    hi_ps = lo_ps if hi == lo else throughput_at(raws, hi)

    print(f"RetiNet {RNS.__version__} / announce-parallel: resolved {resolved}/{len(raws)}, "
          f"{lo}t {lo_ps:.0f}/s, {hi}t {hi_ps:.0f}/s")
    print(f"RESULT resolved={resolved} lo={lo} lo_per_sec={lo_ps:.3f} hi={hi} hi_per_sec={hi_ps:.3f}")


if __name__ == "__main__":
    main()
