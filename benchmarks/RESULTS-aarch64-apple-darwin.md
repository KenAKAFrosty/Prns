# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — Apple M4
- **Cores** — 10 physical / 10 logical
- **Memory** — 16.0 GiB
- **OS** — macOS 26.4
- **Kernel** — 25.4.0

## link-firehose-small-payload (v1)

_The manifest has since moved to v2; every figure below was measured under v1._

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 253,802 / 253,802 | 25.4k msg/s | 7.6 MB/s | 0 / 1 ms | 10.1 / 20.5 MiB | 0.29 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 86,246 / 86,246 | 8.6k msg/s | 2.6 MB/s | 2 / 2 ms | 8.8 / 68.0 MiB | 1.09 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 80,911 / 80,911 | 8.1k msg/s | 2.4 MB/s | 1 / 2 ms | 44.2 / 66.8 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 42,483 / 42,628 · 145 timed out | 3.9k msg/s | 1.2 MB/s | 0 / 1 ms | 55.0 / 9.5 MiB | 1.50 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 50,343 / 50,466 · 123 timed out | 4.7k msg/s | 1.4 MB/s | 1 / 2 ms | 56.6 / 54.1 MiB | 2.14 mJ |
| go-reticulum → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 117,045 / 117,212 · 167 timed out | 11.1k msg/s | 3.3 MB/s | 0 / 0 ms | 56.5 / 13.5 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 15,590 / 15,758 · 168 timed out | 1.5k msg/s | 439 kB/s | 0 / 1 ms | 45.6 / 21.9 MiB | _pending_ |
| Prns → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 6,152 / 26,792 · 20,640 timed out | 615 msg/s | 184 kB/s | 0 / 2 ms | 7.6 / 21.7 MiB | _pending_ |
| go-reticulum → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 3,097 / 3,273 · 176 timed out | 294 msg/s | 88 kB/s | 0 / 0 ms | 22.4 / 21.1 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT

## single-firehose (v1)

_The manifest has since moved to v2; every figure below was measured under v1._

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 132,054 / 132,054 | 13.2k msg/s | 4.0 MB/s | 1 / 2 ms | 9.2 / 14.2 MiB | 0.55 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 41,114 / 41,114 | 4.1k msg/s | 1.2 MB/s | 4 / 4 ms | 8.0 / 53.0 MiB | 1.97 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 12,789 / 12,789 | 1.3k msg/s | 384 kB/s | 3 / 18 ms | 42.1 / 8.0 MiB | 6.44 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 12,425 / 12,425 | 1.2k msg/s | 372 kB/s | 3 / 19 ms | 41.6 / 44.4 MiB | 7.79 mJ |
| go-reticulum → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 113,304 / 113,304 | 11.3k msg/s | 3.4 MB/s | 0 / 0 ms | 26.7 / 13.3 MiB | _pending_ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 42,009 / 42,009 | 4.2k msg/s | 1.3 MB/s | 1 / 1 ms | 23.1 / 52.6 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 12,116 / 12,116 | 1.2k msg/s | 363 kB/s | 3 / 19 ms | 41.9 / 21.5 MiB | _pending_ |
| Prns → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 56,348 / 56,364 | 3.8k msg/s | 1.1 MB/s | 1 / 1 ms | 8.2 / 24.2 MiB | _pending_ |
| go-reticulum → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 2,644 / 2,660 · 16 timed out | 211 msg/s | 63 kB/s | 0 / 0 ms | 21.5 / 21.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT

---

- _Conformance_ — settled clean: every sent message proved within the link's traffic timeout, shown as `delivered / sent`. A ✗ flags messages that timed out — a responder slower than `rtt × 6` misses the deadline by spec, not by fault.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules a node actually pays per delivered message. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env "PATH=$PATH" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
