# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — Apple M4
- **Cores** — 10 physical / 10 logical
- **Memory** — 16.0 GiB
- **OS** — macOS 26.4
- **Kernel** — 25.4.0

## announce-energy (v1)

Sustained announce ingest on all logical cores, measuring energy per announce (the price a battery/solar node actually pays). 2560 distinct signed lxmf.delivery announces, replicated to a working set and looped; throughput here is the sustained average under continuous load.

Energy per announce = (active CPU power − idle baseline) ÷ throughput — it normalizes throughput and is fair across every runtime regardless of GC/JIT/interpreter, because it's the actual joules a user pays. The Ed25519 backend is the controlled variable; conformance confirms every implementation processed the same work. Measured on macOS via `powermetrics` (root), so it reproduces with `sudo`, not the one-command drivers.

| Implementation | Language | Ed25519 backend | Conformance | Throughput | CPU power | Energy / announce |
|----------------|----------|-----------------|-------------|-----------:|---------:|------------------:|
| LXMF-rs 0.2.0 | Rust | ed25519-dalek 2.1 | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 231.7k announce/s | 15.5 W | 67 µJ |
| Prns | Rust | ed25519-dalek 2.2 | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 230.7k announce/s | 15.6 W | 68 µJ |
| Leviculum 0.6.3 | Rust | ed25519-dalek 2.2 | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 230.1k announce/s | 15.7 W | 68 µJ |
| go-reticulum | Go | Go stdlib crypto/ed25519 | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 169.9k announce/s | 20.9 W | 123 µJ |
| rns-cr 0.1.0 ‡ | Crystal | OpenSSL EVP (spider-gazelle/ed25519) | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 120.5k announce/s | 17.5 W | 145 µJ |
| microReticulum † ‡ | C++ | rweather Crypto (portable C++) | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 55.4k announce/s | 21.3 W | 384 µJ |
| RetiNet 0.9.4 | Python | PyCA cryptography / OpenSSL | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 6.3k announce/s | 5.7 W | 901 µJ |
| RNS 1.3.1 _(reference)_ | Python | PyCA cryptography / OpenSSL | <img src="assets/check.svg" width="14" alt="conformant" /> 2560 / 2560 | 6.1k announce/s | 5.6 W | 905 µJ |

† Marked partial / not-yet-feature-complete on the upstream maturity list — included as a data point, not part of the feature-complete tier.

‡ Measured verify-only (parse + Ed25519 verify, no route store) — its store isn't thread-safe; this axis is ~97% verify, so it isolates the dominant work.

Throughput here is the sustained average under continuous all-cores load (the energy denominator). Python runs all-core threads but is GIL-bound, so its all-cores ≈ one core.

**Provenance.**

- **LXMF-rs 0.2.0** — [https://github.com/FreeTAKTeam/LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs) @ `30da190` · EPL-2.0 · 1.96.0 (ac68faa20 2026-05-25)
- **Prns** — [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns) · 1.96.0 (ac68faa20 2026-05-25)
- **Leviculum 0.6.3** — [https://codeberg.org/Lew_Palm/leviculum](https://codeberg.org/Lew_Palm/leviculum) @ `6f366ca` · AGPL-3.0-or-later · 1.96.0 (ac68faa20 2026-05-25)
- **go-reticulum** — [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT · go1.26.4
- **rns-cr 0.1.0** — [https://github.com/jtippett/rns-cr](https://github.com/jtippett/rns-cr) @ `514c309` · MIT · crystal 1.20.2 (preview_mt)
- **microReticulum** — [https://github.com/attermann/microReticulum](https://github.com/attermann/microReticulum) @ `79b8524` · Apache-2.0 · Apple clang version 21.0.0 (clang-2100.1.1.101)
- **RetiNet 0.9.4** — [https://codeberg.org/skyguy/retinet](https://codeberg.org/skyguy/retinet) @ `6039094` · AGPL-3.0-or-later · CPython 3.14.5
- **RNS 1.3.1** — [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License · CPython 3.13.13

---

- _Conformance_ — distinct routes the engine resolves from the corpus (or announces verified, for a verify-only port), against the manifest's expected count.
- _Throughput_ — sustained announces per second under continuous all-cores load (the energy denominator).
- _CPU power_ — average active CPU power over that sustained run.
- _Energy / announce_ — (active power − idle baseline) ÷ throughput; the cross-comparable price paid, sorted ascending.

Regenerate: `energy/build.sh` then `sudo energy/measure.sh` (root, for the power counters) to
refresh `results/`, then `cargo run --bin render_results` to rewrite these tables.
