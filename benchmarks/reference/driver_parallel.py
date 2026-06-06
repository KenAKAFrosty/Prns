"""Drive the RNS 1.3.1 reference over the `announce-parallel` corpus and emit result rows.

The reference side of the parallel scenario: shard the corpus across Python threads, each
running the real RNS announce path (`Packet.unpack` then `Identity.validate_announce`),
best-of-N min wall. Conformance — every route resolved — is the thread-count-independent
correctness check, measured once single-threaded, exactly as `driver.py` does for
announce-256.

PyCA's Ed25519 verify holds the GIL, so threads buy a pure-Python runtime no parallelism
here (≈1×). That's the honest reference point the compiled ports are measured against, not a
harness bug — `multiprocessing` would be the way to actually use more cores, at a much
heavier cost than a thread.

Run:  benchmarks/reference/.venv/bin/python benchmarks/reference/driver_parallel.py
"""

import json
import os
import platform
import subprocess
import threading
import time
from pathlib import Path

import RNS
import RNS.Transport

SCENARIO = "announce-parallel"
VERSION = 1
WARMUP = 3
ITERS = 20
HERE = Path(__file__).resolve().parent
CORPUS = HERE.parent / "scenarios" / SCENARIO / "packets.hex"


def host_slug():
    out = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True)
    for line in out.stdout.splitlines():
        if line.startswith("host: "):
            return line[len("host: ") :].strip()
    raise RuntimeError("could not read host triple from `rustc -vV`")


def decode_corpus():
    return [bytes.fromhex(line) for line in CORPUS.read_text().split()]


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
    raws = decode_corpus()

    resolved = conformance(raws)
    lo, hi = 1, os.cpu_count() or 1
    per_sec = {lo: throughput_at(raws, lo)}
    if hi != lo:
        per_sec[hi] = throughput_at(raws, hi)

    host = host_slug()
    results = HERE.parent / "results" / host / SCENARIO / "rns-1.3.1.jsonl"
    stamp = {
        "implementation": "RNS 1.3.1",
        "commit": f"rns {RNS.__version__}",
        "toolchain": f"CPython {platform.python_version()}",
        "host": host,
    }

    rows = [
        {"scenario": SCENARIO, "scenario_version": VERSION, **stamp,
         "axis": "conformance", "metric": "routes_resolved", "value": float(resolved), "unit": "count"},
    ]
    for threads, value in per_sec.items():
        rows.append({"scenario": SCENARIO, "scenario_version": VERSION, **stamp,
                     "axis": "throughput", "metric": "ingest_announces_per_sec",
                     "value": value, "unit": "announce/s", "threads": threads})

    results.parent.mkdir(parents=True, exist_ok=True)
    results.write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    summary = ", ".join(f"{t}t {v:.0f}/s" for t, v in per_sec.items())
    print(f"RNS 1.3.1 / {SCENARIO} @ {host}: routes {resolved}/{len(raws)}, {summary}")
    print(f"  -> {results.relative_to(HERE.parent)}")


if __name__ == "__main__":
    main()
