# Benchmark results — `x86_64-unknown-linux-gnu`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — 12th Gen Intel(R) Core(TM) i7-1260P
- **Cores** — 12 physical / 16 logical
- **Memory** — 31.0 GiB
- **OS** — Linux (Ubuntu 22.04)
- **Kernel** — 6.8.0-124-generic

## link-firehose-small-payload (v1)

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Rows are ordered by energy per delivered message, the most efficient pairing first; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | _pending_ | 23.9k msg/s | 7.2 MB/s | 0 / 1 ms | 7.1 / 17.1 MiB | 0.86 mJ |
| Prns → RNS 1.3.1 _(ref)_ | _pending_ | 7.1k msg/s | 2.1 MB/s | 2 / 3 ms | 5.7 / 45.3 MiB | 2.85 mJ |
| RNS 1.3.1 _(ref)_ → Prns | _pending_ | 5.2k msg/s | 1.6 MB/s | 0 / 1 ms | 44.7 / 7.7 MiB | 3.06 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | _pending_ | 5.6k msg/s | 1.7 MB/s | 1 / 2 ms | 45.6 / 43.3 MiB | 3.94 mJ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## single-firehose (v1)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Rows are ordered by energy per delivered message, the most efficient pairing first; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | _pending_ | 10.3k msg/s | 3.1 MB/s | 1 / 2 ms | 6.0 / 10.1 MiB | 2.67 mJ |
| Prns → RNS 1.3.1 _(ref)_ | _pending_ | 6.9k msg/s | 2.1 MB/s | 2 / 3 ms | 5.9 / 45.1 MiB | 3.07 mJ |
| RNS 1.3.1 _(ref)_ → Prns | _pending_ | 1.9k msg/s | 585 kB/s | 0 / 1 ms | 33.4 / 6.0 MiB | 8.03 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | _pending_ | 1.9k msg/s | 568 kB/s | 0 / 1 ms | 33.2 / 35.4 MiB | 8.85 mJ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

---

- _Conformance_ — settled clean: every sent message proved within the link's traffic timeout, shown as `delivered / sent`. A ✗ flags messages that timed out — a responder slower than `rtt × 6` misses the deadline by spec, not by fault.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules a node actually pays per delivered message. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env "PATH=$PATH" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
