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

Same wire bytes through each implementation's real parse → Ed25519 verify → store path, best-of-50 min wall time. This axis is ~97% Ed25519 verify, so the ranking is a crypto-backend story; figures are comparable only within this host.

| Implementation | Language | Ed25519 backend | Conformance | Ingest throughput | ×ref |
|----------------|----------|-----------------|-------------|-------------------|------|
| personal-rns | Rust | ed25519-dalek 2.2 | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 47.1k announce/s | 7.0× |
| Leviculum 0.6.3 | Rust | ed25519-dalek 2.2 | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 45.4k announce/s | 6.8× |
| LXMF-rs 0.2.0 | Rust | ed25519-dalek 2.1 | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 43.7k announce/s | 6.5× |
| go-reticulum | Go | Go stdlib crypto/ed25519 | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 35.9k announce/s | 5.3× |
| rns-cr 0.1.0 | Crystal | OpenSSL EVP (spider-gazelle/ed25519) | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 24.6k announce/s | 3.7× |
| microReticulum † | C++ | rweather Crypto (portable C++) | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 9.0k announce/s | 1.3× |
| RNS 1.3.1 _(reference)_ | Python | PyCA cryptography / OpenSSL | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 6.7k announce/s | 1.0× |
| RetiNet 0.9.4 | Python | PyCA cryptography / OpenSSL | <img src="assets/check.svg" width="14" alt="conformant" /> 256 / 256 | 6.6k announce/s | 1.0× |

† Marked partial / not-yet-feature-complete on the upstream maturity list — included as a data point, not part of the feature-complete tier.

**Provenance.**

- **personal-rns** — [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns) · 1.96.0 (ac68faa20 2026-05-25)
- **Leviculum 0.6.3** — [https://codeberg.org/Lew_Palm/leviculum](https://codeberg.org/Lew_Palm/leviculum) @ `6f366ca` · AGPL-3.0-or-later · 1.96.0 (ac68faa20 2026-05-25)
- **LXMF-rs 0.2.0** — [https://github.com/FreeTAKTeam/LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs) @ `30da190` · EPL-2.0 · 1.96.0 (ac68faa20 2026-05-25)
- **go-reticulum** — [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT · go1.26.4
- **rns-cr 0.1.0** — [https://github.com/jtippett/rns-cr](https://github.com/jtippett/rns-cr) @ `514c309` · MIT · crystal 1.20.2
- **microReticulum** — [https://github.com/attermann/microReticulum](https://github.com/attermann/microReticulum) @ `79b8524` · Apache-2.0 · Apple clang version 21.0.0 (clang-2100.1.1.101)
- **RNS 1.3.1** — [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License · CPython 3.13.13
- **RetiNet 0.9.4** — [https://codeberg.org/skyguy/retinet](https://codeberg.org/skyguy/retinet) @ `6039094` · AGPL-3.0-or-later · CPython 3.14.5

## announce-parallel (v1)

Ingest 2560 distinct signed lxmf.delivery announces, sharded evenly across worker threads; each shard runs the real parse → Ed25519 verify → store path on its own fresh engine. Swept single-thread vs all of the host's logical cores.

Best-of-30 min wall time; the two columns are single-threaded and the same corpus sharded across all of this host's logical cores. The announce path is ~97% independent per-announce Ed25519 verify, so it parallelizes cleanly — but a runtime with a global interpreter lock (CPython) can't use the extra cores from threads, so its all-cores figure barely moves, while compiled/JIT runtimes scale with the core count. Figures are comparable only within this host.

| Implementation | Language | Conformance | 1 thread | 10 threads |
|----------------|----------|-------------|--------:|--------:|
| personal-rns | Rust | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 43.8k announce/s | 194.2k announce/s |
| LXMF-rs 0.2.0 | Rust | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 43.6k announce/s | 191.7k announce/s |
| Leviculum 0.6.3 | Rust | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 43.3k announce/s | 190.0k announce/s |
| go-reticulum | Go | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 34.5k announce/s | 150.8k announce/s |
| rns-cr 0.1.0 ‡ | Crystal | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 24.3k announce/s | 104.5k announce/s |
| microReticulum † ‡ | C++ | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 9.1k announce/s | 50.9k announce/s |
| RNS 1.3.1 _(reference)_ | Python | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 6.8k announce/s | 6.7k announce/s |
| RetiNet 0.9.4 | Python | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 6.7k announce/s | 6.6k announce/s |

† Marked partial / not-yet-feature-complete on the upstream maturity list — included as a data point, not part of the feature-complete tier.

‡ Measured verify-only (parse + Ed25519 verify, no route store) — its store isn't thread-safe, so the parallel figure isolates the verify work that dominates this axis.

**Provenance.**

- **personal-rns** — [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns) · 1.96.0 (ac68faa20 2026-05-25)
- **LXMF-rs 0.2.0** — [https://github.com/FreeTAKTeam/LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs) @ `30da190` · EPL-2.0 · 1.96.0 (ac68faa20 2026-05-25)
- **Leviculum 0.6.3** — [https://codeberg.org/Lew_Palm/leviculum](https://codeberg.org/Lew_Palm/leviculum) @ `6f366ca` · AGPL-3.0-or-later · 1.96.0 (ac68faa20 2026-05-25)
- **go-reticulum** — [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT · go1.26.4
- **rns-cr 0.1.0** — [https://github.com/jtippett/rns-cr](https://github.com/jtippett/rns-cr) @ `514c309` · MIT · crystal 1.20.2 (preview_mt)
- **microReticulum** — [https://github.com/attermann/microReticulum](https://github.com/attermann/microReticulum) @ `79b8524` · Apache-2.0 · Apple clang version 21.0.0 (clang-2100.1.1.101)
- **RNS 1.3.1** — [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License · CPython 3.13.13
- **RetiNet 0.9.4** — [https://codeberg.org/skyguy/retinet](https://codeberg.org/skyguy/retinet) @ `6039094` · AGPL-3.0-or-later · CPython 3.14.5

---

- _Conformance_ — distinct routes the engine resolves from the corpus (or announces verified, for a verify-only port), against the manifest's expected count.
- _Ingest throughput_ — best-of-N wall time to parse + verify + store the whole corpus into a fresh engine, as announces per second.
- _×ref_ — throughput relative to the Python reference (`RNS`) on this host.
- _1 thread / N threads_ — for the parallel scenario, ingest throughput single-threaded and sharded across all of this host's logical cores.

Regenerate: run each implementation's driver on this host (`bench_result`, `bench_parallel`,
`reference/driver.py`, `reference/driver_parallel.py`, and the `external/<impl>/run.sh` + `run-mt.sh`
one-command drivers) to refresh `results/`, then `render_results` to rewrite these tables.
