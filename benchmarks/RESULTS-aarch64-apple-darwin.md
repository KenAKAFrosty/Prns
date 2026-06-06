# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — Apple M4
- **Cores** — 10 physical / 10 logical
- **Memory** — 16.0 GiB
- **OS** — macOS 26.4
- **Kernel** — 25.4.0

## announce-256 (v1)

Ingest 256 distinct signed lxmf.delivery announces in order over one interface, then settle 64 ticks.

| Axis | Scope | RNS 1.3.1 | personal-rns |
|------|-------|------|------|
| Conformance | cross-impl | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 |
| Ingest throughput | cross-impl | 6.8k announce/s | 47.7k announce/s |

- **RNS 1.3.1** — rns 1.3.1, CPython 3.13.13, aarch64-apple-darwin
- **personal-rns** — 5f535e3, 1.96.0 (ac68faa20 2026-05-25), aarch64-apple-darwin

---

- _Conformance_ — distinct routes the engine resolves from the corpus, against the manifest's expected count.
- _Ingest throughput_ — best-of-N wall time to ingest the whole corpus into a fresh engine, as announces per second.

Regenerate: run each implementation's driver (`bench_result`, `reference/driver.py`) on this host to
refresh `results/`, then `render_results` to rewrite these tables.
