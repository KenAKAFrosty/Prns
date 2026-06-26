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
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,388,645 / 1,388,645 | 46.3k msg/s | 11.1 MB/s | 0 / 1 ms | 16.8 / 52.7 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 199,897 / 200,008 · 111 timed out | 6.7k msg/s | 1.6 MB/s | 1 / 1 ms | 87.1 / 16.0 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 184,525 / 184,541 · 16 timed out | 6.2k msg/s | 1.5 MB/s | 3 / 4 ms | 7.7 / 80.2 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 184,970 / 185,128 · 158 timed out | 6.0k msg/s | 1.4 MB/s | 1 / 3 ms | 86.1 / 79.9 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## request-response (v1)

Windowed RPC round trips over one link - the network's most common interactive pattern (page fetches, queries, telemetry): each request a varied size, each naming a varied response size it wants back, the handler answering through the engine-gated allow list. Latency is the product; requests per second the capacity.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3,307,890 / 3,307,890 | 110.3k msg/s | _pending_ | 0 / 1 ms | 78.2 / 52.6 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 136,837 / 136,853 · 16 raced | 4.6k msg/s | _pending_ | 1 / 2 ms | 77.4 / 12.3 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 92,499 / 92,499 | 3.1k msg/s | _pending_ | 1 / 2 ms | 11.1 / 58.3 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 76,052 / 76,052 | 2.5k msg/s | _pending_ | 1 / 3 ms | 57.4 / 50.7 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-bulk (v1)

A single large resource transferred whole, over and over, for a fixed wall-time - the multi-segment bulk mechanism. Each logical transfer is 64 MiB sliced into MAX_EFFICIENT_SIZE segments, sent one at a time and proved before the next, so the engine and the host each hold a single segment while the receiver appends the stream to disk-sized totals. Against resource-transfer (one segment) this measures whether the per-byte rate holds past the single-segment ceiling and whether peak memory stays flat at one segment regardless of total size.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 92 / 92 | 3 msg/s | 205.2 MB/s | 325 / 350 ms | 137.8 / 136.6 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 35 / 35 | 1 msg/s | 77.5 MB/s | 840 / 1168 ms | 11.4 / 452.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 24 / 24 | 1 msg/s | 52.9 MB/s | 1274 / 1796 ms | 1030.9 / 388.1 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | 0 msg/s | 17.9 MB/s | 3756 / 3929 ms | 252.6 / 9.4 MiB | _pending_ |

> _The RNS 1.3.5 → Prns row is reference-sender-bound, not engine-bound. RNS prepares each segment lazily on a background thread and naps in 50 ms quanta (`Resource.py`: `while self.next_segment == None: time.sleep(0.05)`) while our receiver — which proves a segment in ~5 ms — waits, so the RNS sender sits idle ~80% of the run. The figure measures CPython's segment-prep pipelining, not the Prns receiver; the receiver's own rate is the Prns → Prns row._

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-transfer (v1)

Sequential maximum-size resource transfers over one link for a fixed wall-time - the bulk mechanism: one sealed stream sliced into parts, pulled by the receiver inside its AIMD window, proved whole by hash. Goodput counts settled transfers' payload bytes; the protocol itself hash-verifies every byte that arrives.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 6,014 / 6,014 | 200 msg/s | 210.2 MB/s | 4 / 5 ms | 73.7 / 72.7 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3,069 / 3,069 | 102 msg/s | 107.3 MB/s | 9 / 12 ms | 320.2 / 9.7 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,218 / 2,218 | 74 msg/s | 77.5 MB/s | 12 / 17 ms | 11.4 / 318.8 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 1,572 / 1,572 | 52 msg/s | 54.9 MB/s | 19 / 23 ms | 191.8 / 236.5 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose (v2)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 845,605 / 845,605 | 28.2k msg/s | 6.2 MB/s | 0 / 1 ms | 11.0 / 43.7 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 183,523 / 183,523 | 6.1k msg/s | 1.3 MB/s | 3 / 3 ms | 6.5 / 80.1 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 65,401 / 65,401 | 2.2k msg/s | 480 kB/s | 0 / 1 ms | 42.2 / 46.6 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 62,770 / 62,770 | 2.1k msg/s | 461 kB/s | 0 / 1 ms | 41.9 / 8.6 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose-256 (v2)

The single-firehose at a deep window (256 in flight): a ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time. The deep variant surfaces what shallow windows hide - whether inbound decrypt/verify serialize on the reactor or parallelize off it.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 873,399 / 873,399 | 29.1k msg/s | 6.4 MB/s | 5 / 10 ms | 11.5 / 48.9 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 186,625 / 186,625 | 6.2k msg/s | 1.4 MB/s | 41 / 45 ms | 6.8 / 79.9 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 65,286 / 65,286 | 2.2k msg/s | 479 kB/s | 0 / 0 ms | 42.9 / 46.7 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 61,317 / 61,317 | 2.0k msg/s | 450 kB/s | 0 / 1 ms | 42.4 / 8.6 MiB | _pending_ |

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
