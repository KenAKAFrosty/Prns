# Benchmark results — `x86_64-pc-windows-msvc`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — AMD Ryzen 5 5600X 6-Core Processor
- **Cores** — 6 physical / 12 logical
- **Memory** — 31.9 GiB
- **OS** — Windows 11 Home
- **Kernel** — 26200

## link-firehose-small-payload (v2)

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,097,987 / 1,097,987 | 36.6k msg/s | 8.8 MB/s | 0 / 1 ms | 0.0 / 0.0 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 128,991 / 128,991 | 4.3k msg/s | 1.0 MB/s | 4 / 4 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 124,196 / 124,196 | 4.1k msg/s | 993 kB/s | 3 / 4 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 27,388 / 27,853 · 465 timed out | 888 msg/s | 213 kB/s | 0 / 0 ms | 0.0 / 0.0 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-bulk (v1)

_The manifest has since moved to v2; every figure below was measured under v1._

A single large resource transferred whole, over and over, for a fixed wall-time - the multi-segment bulk mechanism. Each logical transfer is 64 MiB sliced into MAX_EFFICIENT_SIZE segments, sent one at a time and proved before the next, so the engine and the host each hold a single segment while the receiver appends the stream to disk-sized totals. Against resource-transfer (one segment) this measures whether the per-byte rate holds past the single-segment ceiling and whether peak memory stays flat at one segment regardless of total size. Compression is off on BOTH stacks (the reference harness has always passed auto_compress=False), so this row is the pure transport rate; the codec-engaged posture is resource-bulk-compressed.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 84 / 84 | 3 msg/s | 186.0 MB/s | 357 / 395 ms | 0.0 / 0.0 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 26 / 26 | 1 msg/s | 57.1 MB/s | 1205 / 1772 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 23 / 23 | 1 msg/s | 50.9 MB/s | 1263 / 1845 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | 0 msg/s | 15.9 MB/s | 4240 / 4489 ms | 0.0 / 0.0 MiB | _pending_ |

> _The RNS 1.3.5 → Prns row is reference-sender-bound, not engine-bound. RNS prepares each segment lazily on a background thread and naps in 50 ms quanta (`Resource.py`: `while self.next_segment == None: time.sleep(0.05)`) while our receiver — which proves a segment in ~5 ms — waits, so the RNS sender sits idle ~80% of the run. The figure measures CPython's segment-prep pipelining, not the Prns receiver; the receiver's own rate is the Prns → Prns row._

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-transfer (v1)

Sequential maximum-size resource transfers over one link for a fixed wall-time - the bulk mechanism: one sealed stream sliced into parts, pulled by the receiver inside its AIMD window, proved whole by hash. Goodput counts settled transfers' payload bytes; the protocol itself hash-verifies every byte that arrives.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 5,327 / 5,327 | 178 msg/s | 186.2 MB/s | 5 / 12 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2,605 / 2,605 | 87 msg/s | 91.0 MB/s | 11 / 13 ms | 0.0 / 0.0 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,046 / 2,046 | 68 msg/s | 71.5 MB/s | 14 / 26 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 1,519 / 1,519 | 51 msg/s | 53.1 MB/s | 19 / 22 ms | 0.0 / 0.0 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## single-firehose (v2)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 812,187 / 812,187 | 27.1k msg/s | 6.0 MB/s | 0 / 1 ms | 0.0 / 0.0 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 198,735 / 198,735 | 6.6k msg/s | 1.5 MB/s | 2 / 4 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 103,028 / 103,028 | 3.4k msg/s | 755 kB/s | 0 / 1 ms | 0.0 / 0.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 100,826 / 100,826 | 3.4k msg/s | 739 kB/s | 0 / 0 ms | 0.0 / 0.0 MiB | _pending_ |

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
