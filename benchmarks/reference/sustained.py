"""RNS' sustained harness for the `announce-energy` scenario: run the real announce path
(`Packet.unpack` + `Identity.validate_announce`) under continuous load for a fixed wall-time,
so powermetrics can integrate package power over a long steady run. Prints sustained
throughput; `energy/measure.sh` wraps the power sampling around it.

Runs at all logical cores (uniform "full send" with the compiled ports) — but PyCA's Ed25519
verify holds the GIL, so the extra threads buy no parallelism; this is the honest outcome of
asking Python to use the machine. RetiNet (an RNS fork) reuses this same script via its venv.

Run: <venv>/bin/python reference/sustained.py <corpus.hex> <seconds> [threads]
"""

import os
import sys
import threading
import time

import RNS
import RNS.Transport


def decode(path):
    return [bytes.fromhex(line) for line in open(path).read().split()]


def ingest(chunk, deadline, counter, idx):
    n = 0
    while time.perf_counter() < deadline:
        RNS.Identity.known_destinations = {}
        for raw in chunk:
            packet = RNS.Packet(None, raw)
            packet.unpack()
            RNS.Identity.validate_announce(packet)
        n += len(chunk)
    counter[idx] = n


def main():
    corpus = sys.argv[1]
    secs = float(sys.argv[2]) if len(sys.argv) > 2 else 60.0
    threads = int(sys.argv[3]) if len(sys.argv) > 3 else (os.cpu_count() or 1)

    base = decode(corpus)

    RNS.Identity.known_destinations = {}
    resolved = 0
    for raw in base:
        packet = RNS.Packet(None, raw)
        packet.unpack()
        if RNS.Identity.validate_announce(packet):
            resolved += 1
    print(f"CONFORMANCE resolved={resolved}")

    chunk_size = (len(base) + threads - 1) // threads
    shards = [base[i : i + chunk_size] for i in range(0, len(base), chunk_size)]

    deadline = time.perf_counter() + secs
    start = time.perf_counter()
    counter = [0] * len(shards)
    workers = [threading.Thread(target=ingest, args=(s, deadline, counter, i)) for i, s in enumerate(shards)]
    for w in workers:
        w.start()
    for w in workers:
        w.join()
    elapsed = time.perf_counter() - start
    total = sum(counter)
    print(f"THROUGHPUT announces_per_sec={total / elapsed:.1f} total={total} secs={elapsed:.2f}")


if __name__ == "__main__":
    main()
