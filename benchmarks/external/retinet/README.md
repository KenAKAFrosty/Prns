# RetiNet — announce-256 driver

Measures [RetiNet](https://codeberg.org/skyguy/retinet) (an AGPL Python fork of RNS, ships
the `RNS` module) on the shared `announce-256` corpus: `Packet.unpack` +
`Identity.validate_announce`, best-of-50 min wall time, `known_destinations` reset each
pass. Same path and methodology as the RNS reference (`benchmarks/reference/driver.py`).

## Run

```sh
./run.sh
```

Needs `python3`. Clones the pinned upstream into `.upstream/` (gitignored), installs it
into an **isolated venv** (it ships `RNS`, so it must not collide with a system RNS), runs
the driver, and writes `../../results/<host>/announce-256/retinet.jsonl`.

- **Upstream:** https://codeberg.org/skyguy/retinet @ `6039094` (rns 0.9.4)
- **License:** AGPL-3.0-or-later — we vendor only `driver.py` (our code) + the numbers.
- **Crypto backend:** PyCA cryptography / OpenSSL (same as the RNS reference).
