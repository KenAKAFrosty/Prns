# rns-cr — announce-256 driver

Measures [rns-cr](https://github.com/jtippett/rns-cr) (Crystal) on the shared
`announce-256` corpus: `Packet#unpack` + `Identity.validate_announce` (the Ed25519
verify + store), best-of-50 min wall time, `known_destinations` cleared each pass so
every pass does the full verify work.

Notable: rns-cr's Ed25519 is the **same OpenSSL EVP** the Python reference uses (via the
`spider-gazelle/ed25519` shard), but compiled — so it isolates the Python interpreter's
overhead from the crypto cost.

## Run

```sh
./run.sh
```

Needs Crystal (≥ 1.9) + OpenSSL on `PATH`. Clones the pinned upstream into `.upstream/`
(gitignored), runs `shards install`, drops `bench.cr` into the repo root (so
`require "./src/rns"` resolves), `crystal run --release`s it, and writes
`../../results/<host>/announce-256/rns-cr.jsonl`.

- **Upstream:** https://github.com/jtippett/rns-cr @ `514c309` (v0.1.0)
- **License:** MIT — we vendor only `bench.cr` (our code) + the numbers.
- **Crypto backend:** OpenSSL EVP via spider-gazelle/ed25519.
