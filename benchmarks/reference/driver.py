"""Drive the RNS 1.3.1 reference over the `announce-256` corpus and emit result rows.

The benchmark table compares implementations on the figures that are fair to compare:
conformance (does it resolve every route?) and ingest throughput. This is the reference
side — it replays the exact wire bytes every other implementation replays, through the
real RNS announce path: `Packet.unpack` then `Identity.validate_announce`, which recovers
the announced identity, verifies the Ed25519 signature, and remembers the destination.

No live Reticulum: `validate_announce` works off class state (`Identity.known_destinations`,
`Transport.blackholed_identities`), so nothing here touches the network. Each timed pass
resets `known_destinations` so the work is identical across iterations.

Run:  benchmarks/reference/.venv/bin/python benchmarks/reference/driver.py
"""

import json
import platform
import time
from pathlib import Path

import RNS
import RNS.Transport

SCENARIO = "announce-256"
VERSION = 1
WARMUP = 3
ITERS = 20
HERE = Path(__file__).resolve().parent
CORPUS = HERE.parent / "scenarios" / SCENARIO / "packets.hex"
RESULTS = HERE.parent / "results" / SCENARIO / "rns-1.3.1.jsonl"


def decode_corpus():
    return [bytes.fromhex(line) for line in CORPUS.read_text().split()]


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
    raws = decode_corpus()
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

    stamp = {
        "implementation": "RNS 1.3.1",
        "commit": f"rns {RNS.__version__}",
        "toolchain": f"CPython {platform.python_version()}",
        "host": f"{platform.machine()}-{platform.system().lower()}",
    }

    def row(axis, metric, value, unit):
        return {
            "scenario": SCENARIO,
            "scenario_version": VERSION,
            **stamp,
            "axis": axis,
            "metric": metric,
            "value": value,
            "unit": unit,
        }

    rows = [
        row("conformance", "routes_resolved", float(resolved), "count"),
        row("throughput", "ingest_announces_per_sec", per_sec, "announce/s"),
    ]
    RESULTS.parent.mkdir(parents=True, exist_ok=True)
    RESULTS.write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    print(f"RNS 1.3.1 / {SCENARIO}: routes {resolved}/{count}, ingest {per_sec:.0f} announce/s")
    print(f"  -> {RESULTS.relative_to(HERE.parent)}")


if __name__ == "__main__":
    main()
