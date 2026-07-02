#!/usr/bin/env python3
"""Real-RNS interop smoke client for the local shared-instance interface.

Connects to a running Prns ``LocalServer`` as a stock ``RNS.Reticulum`` shared-instance client (the
pinned reference install): with a fresh config dir and the default ``share_instance = True``, RNS tries
to bind the loopback shared-instance port, fails because the Prns daemon already holds it, and so
connects to it as a local client instead — the exact path Sideband/NomadNet/MeshChat take. It then
announces a destination, which crosses the connection to the Prns daemon as a genuine RNS announce.

Prints ``ANNOUNCED dest=<hex>`` on stdout so the runner can match it against the daemon's ``HEARD``
line. RNS's own logs go to stderr, leaving stdout clean for the one machine-readable line.
"""

import sys
import tempfile
import time

import RNS


def main() -> int:
    configdir = tempfile.mkdtemp(prefix="rns-smoke-")
    # Quiet RNS's own chatter to stderr; stdout carries only our ANNOUNCED line.
    RNS.Reticulum(configdir=configdir, loglevel=RNS.LOG_WARNING)
    # Let the client settle its connection to the shared instance before announcing.
    time.sleep(1.5)

    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "personal",
        "smoke",
    )
    print("ANNOUNCED dest=" + destination.hash.hex(), flush=True)

    # Announce a handful of times; the daemon only needs to hear one.
    for _ in range(12):
        destination.announce()
        time.sleep(0.5)
    time.sleep(1.0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
