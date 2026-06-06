"""Drive RetiNet (an RNS fork; ships the `RNS` module) over the shared announce-256 corpus
through Packet.unpack + Identity.validate_announce, best-of-N min wall time, reset
known_destinations each pass. Mirrors benchmarks/reference/driver.py. run.sh runs this with
RetiNet installed in an isolated venv. Prints a `RESULT resolved=<n> per_sec=<f>` line."""

import sys
import time

import RNS
import RNS.Transport

WARMUP = 5
ITERS = 50


def decode(path):
    return [bytes.fromhex(line) for line in open(path).read().split()]


def ingest(raws):
    RNS.Identity.known_destinations = {}
    resolved = 0
    for raw in raws:
        packet = RNS.Packet(None, raw)
        packet.unpack()
        if RNS.Identity.validate_announce(packet):
            resolved += 1
    return resolved


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: driver.py <corpus.hex>")
    raws = decode(sys.argv[1])
    count = len(raws)

    resolved = ingest(raws)

    best = float("inf")
    for i in range(WARMUP + ITERS):
        start = time.perf_counter()
        ingest(raws)
        secs = time.perf_counter() - start
        if i >= WARMUP:
            best = min(best, secs)
    per_sec = count / best

    print(f"RetiNet {RNS.__version__} / announce-256: resolved {resolved}/{count}, {per_sec:.0f} announce/s")
    print(f"RESULT resolved={resolved} per_sec={per_sec:.3f}")


if __name__ == "__main__":
    main()
