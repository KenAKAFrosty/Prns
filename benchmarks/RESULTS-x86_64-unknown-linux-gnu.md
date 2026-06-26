# Benchmark results — `x86_64-unknown-linux-gnu`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — 12th Gen Intel(R) Core(TM) i7-1260P
- **Cores** — 12 physical / 16 logical
- **Memory** — 31.0 GiB
- **OS** — Linux (Ubuntu 22.04)
- **Kernel** — 6.8.0-124-generic

## link-firehose-small-payload (v2)

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 661,131 / 661,131 | 22.0k msg/s | 5.3 MB/s | 0 / 1 ms | 11.1 / 36.1 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 204,066 / 204,082 · 16 timed out | 6.8k msg/s | 1.6 MB/s | 2 / 3 ms | 7.5 / 81.2 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 170,610 / 170,610 | 5.7k msg/s | 1.4 MB/s | 2 / 3 ms | 86.8 / 12.9 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 168,293 / 168,294 · 1 timed out | 5.6k msg/s | 1.3 MB/s | 2 / 3 ms | 86.7 / 80.8 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## request-response (v1)

Windowed RPC round trips over one link - the network's most common interactive pattern (page fetches, queries, telemetry): each request a varied size, each naming a varied response size it wants back, the handler answering through the engine-gated allow list. Latency is the product; requests per second the capacity.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2,880,667 / 2,880,667 | 96.0k msg/s | _pending_ | 0 / 1 ms | 74.9 / 52.5 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 133,946 / 133,976 · 30 raced | 4.5k msg/s | _pending_ | 1 / 1 ms | 75.4 / 12.4 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 125,738 / 125,738 | 4.2k msg/s | _pending_ | 1 / 2 ms | 12.8 / 63.4 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 119,683 / 119,683 | 4.0k msg/s | _pending_ | 1 / 1 ms | 71.6 / 62.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-bulk (v1)

A single large resource transferred whole, over and over, for a fixed wall-time - the multi-segment bulk mechanism. Each logical transfer is 64 MiB sliced into MAX_EFFICIENT_SIZE segments, sent one at a time and proved before the next, so the engine and the host each hold a single segment while the receiver appends the stream to disk-sized totals. Against resource-transfer (one segment) this measures whether the per-byte rate holds past the single-segment ceiling and whether peak memory stays flat at one segment regardless of total size.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 99 / 99 | 3 msg/s | 219.4 MB/s | 304 / 335 ms | 137.6 / 136.6 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 39 / 39 | 1 msg/s | 85.2 MB/s | 766 / 1055 ms | 11.8 / 500.4 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 37 / 37 | 1 msg/s | 81.9 MB/s | 768 / 1279 ms | 1058.4 / 480.1 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 9 / 9 | 0 msg/s | 19.9 MB/s | 3374 / 3871 ms | 234.9 / 9.0 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-transfer (v1)

Sequential maximum-size resource transfers over one link for a fixed wall-time - the bulk mechanism: one sealed stream sliced into parts, pulled by the receiver inside its AIMD window, proved whole by hash. Goodput counts settled transfers' payload bytes; the protocol itself hash-verifies every byte that arrives.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 6,326 / 6,326 | 211 msg/s | 221.1 MB/s | 4 / 5 ms | 73.6 / 72.6 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3,365 / 3,365 | 112 msg/s | 117.6 MB/s | 9 / 11 ms | 343.1 / 9.5 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,468 / 2,468 | 82 msg/s | 86.3 MB/s | 11 / 15 ms | 11.5 / 352.3 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 1,854 / 1,854 | 62 msg/s | 64.8 MB/s | 16 / 19 ms | 216.9 / 283.0 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose (v2)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 226,112 / 226,112 | 7.5k msg/s | 1.7 MB/s | 1 / 3 ms | 7.2 / 16.5 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 194,740 / 194,740 | 6.5k msg/s | 1.4 MB/s | 2 / 3 ms | 6.9 / 80.8 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 59,466 / 59,466 | 2.0k msg/s | 436 kB/s | 0 / 4 ms | 41.5 / 45.8 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 45,900 / 45,900 | 1.5k msg/s | 337 kB/s | 0 / 9 ms | 39.9 / 8.1 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose-256 (v2)

The single-firehose at a deep window (256 in flight): a ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time. The deep variant surfaces what shallow windows hide - whether inbound decrypt/verify serialize on the reactor or parallelize off it.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 229,101 / 229,101 | 7.6k msg/s | 1.7 MB/s | 17 / 21 ms | 7.4 / 16.7 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 192,690 / 192,690 | 6.4k msg/s | 1.4 MB/s | 40 / 45 ms | 7.0 / 80.5 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 56,431 / 56,431 | 1.9k msg/s | 414 kB/s | 0 / 6 ms | 41.8 / 45.4 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 52,815 / 52,815 | 1.8k msg/s | 387 kB/s | 0 / 9 ms | 41.5 / 8.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

---

- _Conformance_ — every sent message accounted for, shown as `delivered / sent`. Extra suffixes call out messages that timed out or landed in a scenario-declared `raced` bucket, such as the RNS 1.3.5 request-response send-before-register loopback race.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules per delivered message. The power counters are package-domain, so this is the *combined* cost of both roles on the SoC; only the diagonal (a self-pair) is a single impl. The `i … / r …` split apportions it to initiator vs responder by their CPU-time share — the honest cross-platform proxy (Linux RAPL has no per-process counter), exact only insofar as power tracks CPU time. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env "PATH=$PATH" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
