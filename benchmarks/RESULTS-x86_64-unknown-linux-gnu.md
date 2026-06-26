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
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,379,174 / 1,379,174 | 46.0k msg/s | 11.0 MB/s | 0 / 1 ms | 16.7 / 52.7 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 193,582 / 193,700 · 118 timed out | 6.5k msg/s | 1.5 MB/s | 1 / 2 ms | 86.2 / 16.0 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 187,402 / 187,418 · 16 timed out | 6.2k msg/s | 1.5 MB/s | 2 / 4 ms | 7.9 / 80.4 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 185,845 / 185,978 · 133 timed out | 6.1k msg/s | 1.5 MB/s | 1 / 3 ms | 85.8 / 79.9 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## request-response (v1)

Windowed RPC round trips over one link - the network's most common interactive pattern (page fetches, queries, telemetry): each request a varied size, each naming a varied response size it wants back, the handler answering through the engine-gated allow list. Latency is the product; requests per second the capacity.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3,355,591 / 3,355,591 | 111.9k msg/s | _pending_ | 0 / 1 ms | 78.5 / 52.8 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 136,689 / 136,708 · 19 raced | 4.6k msg/s | _pending_ | 1 / 2 ms | 77.2 / 12.4 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 92,914 / 92,914 | 3.1k msg/s | _pending_ | 1 / 2 ms | 11.2 / 58.1 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 76,399 / 76,399 | 2.5k msg/s | _pending_ | 1 / 3 ms | 57.8 / 50.7 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-bulk (v1)

A single large resource transferred whole, over and over, for a fixed wall-time - the multi-segment bulk mechanism. Each logical transfer is 64 MiB sliced into MAX_EFFICIENT_SIZE segments, sent one at a time and proved before the next, so the engine and the host each hold a single segment while the receiver appends the stream to disk-sized totals. Against resource-transfer (one segment) this measures whether the per-byte rate holds past the single-segment ceiling and whether peak memory stays flat at one segment regardless of total size.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 92 / 92 | 3 msg/s | 204.2 MB/s | 327 / 351 ms | 137.9 / 136.9 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 35 / 35 | 1 msg/s | 77.1 MB/s | 866 / 1112 ms | 11.5 / 450.8 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 24 / 24 | 1 msg/s | 53.4 MB/s | 1239 / 2038 ms | 1037.4 / 390.7 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | 0 msg/s | 17.7 MB/s | 3771 / 4240 ms | 236.9 / 9.3 MiB | _pending_ |

> _The RNS 1.3.5 → Prns row is reference-sender-bound, not engine-bound. RNS prepares each segment lazily on a background thread and naps in 50 ms quanta (`Resource.py`: `while self.next_segment == None: time.sleep(0.05)`) while our receiver — which proves a segment in ~5 ms — waits, so the RNS sender sits idle ~80% of the run. The figure measures CPython's segment-prep pipelining, not the Prns receiver; the receiver's own rate is the Prns → Prns row._

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-transfer (v1)

Sequential maximum-size resource transfers over one link for a fixed wall-time - the bulk mechanism: one sealed stream sliced into parts, pulled by the receiver inside its AIMD window, proved whole by hash. Goodput counts settled transfers' payload bytes; the protocol itself hash-verifies every byte that arrives.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 5,967 / 5,967 | 199 msg/s | 208.5 MB/s | 4 / 5 ms | 73.8 / 72.9 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3,079 / 3,079 | 103 msg/s | 107.6 MB/s | 10 / 12 ms | 320.4 / 9.7 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,153 / 2,153 | 72 msg/s | 75.2 MB/s | 13 / 17 ms | 11.4 / 330.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 1,510 / 1,510 | 50 msg/s | 52.8 MB/s | 19 / 24 ms | 190.8 / 248.5 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose (v2)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 704,702 / 704,702 | 23.5k msg/s | 5.2 MB/s | 1 / 3 ms | 10.4 / 39.5 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 179,165 / 179,165 | 6.0k msg/s | 1.3 MB/s | 3 / 3 ms | 6.1 / 80.6 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 64,422 / 64,422 | 2.1k msg/s | 473 kB/s | 0 / 1 ms | 42.1 / 46.6 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 63,391 / 63,391 | 2.1k msg/s | 465 kB/s | 0 / 1 ms | 41.9 / 8.8 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose-256 (v2)

The single-firehose at a deep window (256 in flight): a ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time. The deep variant surfaces what shallow windows hide - whether inbound decrypt/verify serialize on the reactor or parallelize off it.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,020,466 / 1,020,466 | 34.0k msg/s | 7.5 MB/s | 6 / 11 ms | 13.4 / 53.5 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 187,211 / 187,211 | 6.2k msg/s | 1.4 MB/s | 40 / 46 ms | 6.8 / 79.3 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 65,937 / 65,937 | 2.2k msg/s | 484 kB/s | 0 / 0 ms | 43.0 / 46.9 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 62,407 / 62,407 | 2.1k msg/s | 458 kB/s | 0 / 1 ms | 42.6 / 8.8 MiB | _pending_ |

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
