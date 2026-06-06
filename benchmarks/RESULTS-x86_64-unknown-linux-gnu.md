# Benchmark results — `x86_64-unknown-linux-gnu`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — _pending_
- **Cores** — _pending_
- **Memory** — _pending_
- **OS** — _pending_
- **Kernel** — _pending_

## announce-256 (v1)

Ingest 256 distinct signed lxmf.delivery announces in order over one interface, then settle 64 ticks.

Same wire bytes through each implementation's real parse → Ed25519 verify → store path, best-of-50 min wall time. This axis is ~97% Ed25519 verify, so the ranking is a crypto-backend story; figures are comparable only within this host.

| Implementation | Language | Ed25519 backend | Conformance | Ingest throughput | ×ref |
|----------------|----------|-----------------|-------------|-------------------|------|
| Prns | Rust | ed25519-dalek 2.2 | _pending_ | _pending_ | — |
| RNS 1.3.1 _(reference)_ | Python | PyCA cryptography / OpenSSL | _pending_ | _pending_ | — |

**Provenance.**

- **Prns** — [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns) · pending
- **RNS 1.3.1** — [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License · pending

---

- _Conformance_ — distinct routes the engine resolves from the corpus (or announces verified, for a verify-only port), against the manifest's expected count.
- _Ingest throughput_ — best-of-N wall time to parse + verify + store the whole corpus into a fresh engine, as announces per second.
- _×ref_ — throughput relative to the Python reference (`RNS`) on this host.
- _1 thread / N threads_ — for the parallel scenario, ingest throughput single-threaded and sharded across all of this host's logical cores.

Regenerate: run each implementation's driver on this host (`bench_result`, `bench_parallel`,
`reference/driver.py`, `reference/driver_parallel.py`, and the `external/<impl>/run.sh` + `run-mt.sh`
one-command drivers) to refresh `results/`, then `render_results` to rewrite these tables.
