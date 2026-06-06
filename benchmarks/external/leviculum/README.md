# Leviculum — announce-256 driver

Measures [Leviculum](https://codeberg.org/Lew_Palm/leviculum) (Rust, `reticulum-core`)
on the shared `announce-256` corpus, through its real ingest path
`Transport::process_incoming` (parse + Ed25519 verify + store a path), best-of-50 min
wall time. Conformance is `Transport::path_count()` after one pass.

## Run

```sh
./run.sh
```

It clones the pinned upstream into `.upstream/` (gitignored — never committed), builds
`harness/` against its `reticulum-core`, runs it over `../../scenarios/announce-256/packets.hex`,
and writes `../../results/<host>/announce-256/leviculum.jsonl` in the shared result schema.

- **Upstream:** https://codeberg.org/Lew_Palm/leviculum @ `6f366ca` (v0.6.3)
- **License:** AGPL-3.0-or-later — we vendor only `harness/` (our code) and the result
  numbers, never upstream source.
- **Crypto backend:** ed25519-dalek 2.2 (this axis is ~97% Ed25519 verify).
