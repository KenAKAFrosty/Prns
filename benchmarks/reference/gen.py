"""Mint the `announce-256` scenario corpus from the RNS 1.3.1 reference.

The benchmark corpus is the fairness/conformance gate: every implementation replays
these exact wire bytes. So the bytes must be *reference* ground truth, not minted by
the engine-under-test. This script is the canonical generator — it drives the real
`RNS.Destination.announce` to produce each packet, then writes `packets.hex`.

Determinism (so regeneration reproduces, and so the engine's own `gen_corpus` can diff
against it for byte-exact parity): RNS draws one nonce per announce,

    random_hash = Identity.get_random_hash()[0:5] + int(time.time()).to_bytes(5, "big")

both halves of which are pinned below to a per-index deterministic value. Identities are
loaded from deterministic secret bytes. Nothing here touches the network or a live
Reticulum instance — `announce(send=False)` returns the packet and `pack()` serializes it.

Run:  benchmarks/reference/.venv/bin/python benchmarks/reference/gen.py [--check]
"""

import sys
from contextlib import contextmanager
from pathlib import Path

import RNS
import RNS.Destination
import RNS.Transport

APP_NAME = "lxmf"
ASPECTS = ["delivery"]
APP_DATA = b"benchmarks"
SCENARIOS_DIR = Path(__file__).resolve().parent.parent / "scenarios"
SCENARIOS = [("announce-256", 256), ("announce-parallel", 2560)]

# A live Reticulum only matters for routing/transport, never for the announce bytes.
RNS.Transport.register_destination = staticmethod(lambda *a, **k: None)


def node_secret(index):
    seed = (index ^ 0xC300) & 0xFFFF
    lo, hi = seed & 0xFF, (seed >> 8) & 0xFF
    block = index >> 8
    return bytes((lo * 31 + hi + i + 1 + block * i) & 0xFF for i in range(64))


def announce_nonce(index):
    random_half = bytes([(0x40 + index) & 0xFF]) * 5
    time_half = (1000).to_bytes(5, "big")
    return random_half, time_half


@contextmanager
def pinned_nonce(random_half, time_half):
    saved_random = RNS.Identity.get_random_hash
    saved_time = sys.modules["RNS.Destination"].time.time
    RNS.Identity.get_random_hash = staticmethod(lambda: random_half + bytes(27))
    sys.modules["RNS.Destination"].time.time = lambda: int.from_bytes(time_half, "big")
    try:
        yield
    finally:
        RNS.Identity.get_random_hash = saved_random
        sys.modules["RNS.Destination"].time.time = saved_time


def announce_packet(index):
    random_half, time_half = announce_nonce(index)
    identity = RNS.Identity.from_bytes(node_secret(index))
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, APP_NAME, *ASPECTS
    )
    with pinned_nonce(random_half, time_half):
        packet = destination.announce(app_data=APP_DATA, send=False)
        packet.pack()
    return packet.raw


def corpus_hex(count):
    return [announce_packet(index).hex() for index in range(count)]


def check(name, count):
    lines = corpus_hex(count)
    committed = (SCENARIOS_DIR / name / "packets.hex").read_text().split()
    ok = committed == lines
    print(f"{name}: reference parity {'IDENTICAL' if ok else 'DIVERGES'} ({len(lines)} packets)")
    if not ok:
        first = next(i for i in range(min(len(committed), len(lines))) if committed[i] != lines[i])
        print(f"  first divergence at packet {first}")
        print(f"  committed: {committed[first][:64]}…")
        print(f"  reference: {lines[first][:64]}…")
    return ok


def write(name, count):
    lines = corpus_hex(count)
    target = SCENARIOS_DIR / name / "packets.hex"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(lines) + "\n")
    print(f"wrote {len(lines)} reference packets to {target}")


def main():
    checking = "--check" in sys.argv
    ok = True
    for name, count in SCENARIOS:
        if checking:
            ok &= check(name, count)
        else:
            write(name, count)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
