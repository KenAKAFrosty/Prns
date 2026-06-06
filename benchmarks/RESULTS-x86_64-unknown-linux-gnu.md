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

| Axis | Scope | RNS 1.3.1 | personal-rns |
|------|-------|------|------|
| Conformance | cross-impl | _pending_ | _pending_ |
| Ingest throughput | cross-impl | _pending_ | _pending_ |

- **RNS 1.3.1** — pending, pending, x86_64-unknown-linux-gnu
- **personal-rns** — pending, pending, x86_64-unknown-linux-gnu

---

- _Conformance_ — distinct routes the engine resolves from the corpus, against the manifest's expected count.
- _Ingest throughput_ — best-of-N wall time to ingest the whole corpus into a fresh engine, as announces per second.

Regenerate: run each implementation's driver (`bench_result`, `reference/driver.py`) on this host to
refresh `results/`, then `render_results` to rewrite these tables.
