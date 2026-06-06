# LXMF-rs — announce-256 driver

Measures [LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs)'s `reticulum-rs-core` (Rust)
on the shared `announce-256` corpus: `Packet::from_bytes` → `DestinationAnnounce::validate`
(the Ed25519 verify) → store the recovered destination, best-of-50 min wall time.
`validate()` verifies but doesn't store (storage lives in a separate crate), so the
harness adds a `HashSet` insert to match the parse+verify+store the other impls do.

## Run

```sh
./run.sh
```

Clones the pinned upstream into `.upstream/` (gitignored), drops `announce_bench.rs` into
its `crates/libs/rns-core/examples/` (so the workspace inheritance resolves), runs it, and
writes `../../results/<host>/announce-256/lxmf-rs.jsonl`.

- **Upstream:** https://github.com/FreeTAKTeam/LXMF-rs @ `30da190` (rns-core v0.2.0)
- **License:** EPL-2.0 — we vendor only `announce_bench.rs` (our code) + the numbers.
- **Crypto backend:** ed25519-dalek 2.1 (`verify_strict`).

## Parallel scenario

`./run-mt.sh` measures the `announce-parallel` scenario — 2560 distinct announces sharded
across `[1, cpu_count]` worker threads (the stateless `validate` per shard, deduped to a route
count), single-thread vs all logical cores — and writes
`../../results/<host>/announce-parallel/lxmf-rs.jsonl`.
