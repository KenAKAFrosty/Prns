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
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 722,437 / 722,437 | 24.1k msg/s | 5.8 MB/s | 0 / 1 ms | 11.1 / 41.7 MiB | 0.98 mJ |
| go-reticulum → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 16,393 / 16,888 · 495 timed out | 536 msg/s | 129 kB/s | 0 / 0 ms | 38.9 / 18.3 MiB | 0.30 mJ |
| go-reticulum → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 325,264 / 325,736 · 472 timed out | 10.6k msg/s | 2.5 MB/s | 0 / 0 ms | 92.8 / 21.9 MiB | 1.99 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 201,710 / 202,315 · 605 timed out | 6.7k msg/s | 1.6 MB/s | 2 / 3 ms | 7.2 / 80.2 MiB | 2.63 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 209,812 / 209,825 · 13 timed out | 7.0k msg/s | 1.7 MB/s | 2 / 3 ms | 50.8 / 80.9 MiB | 3.24 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 177,077 / 177,078 · 1 timed out | 5.9k msg/s | 1.4 MB/s | 2 / 2 ms | 85.7 / 14.3 MiB | 3.39 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 170,266 / 170,267 · 1 timed out | 5.7k msg/s | 1.4 MB/s | 2 / 3 ms | 85.7 / 80.0 MiB | 4.38 mJ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 1,490 / 6,813,767 · 6,812,277 timed out | 50 msg/s | 12 kB/s | 1 / 1 ms | 32.3 / 17.5 MiB | 229.77 mJ |
| Prns → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 6,728 / 6,871 · 143 timed out | _pending_ | _pending_ | _pending_ | 5.5 / 18.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT

## request-response-degraded-bluetooth (v1)

Windowed RPC through a shaped pipe at a degraded-Bluetooth wire (100 kbps, 30 ms one-way): the interactive pattern under the constraint a phone-to-node hop actually has. Latency is now wire-dominated instead of engine-dominated, so the product is how little the protocol adds on top of the physics.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 967 / 967 | 32 msg/s | _pending_ | 120 / 194 ms | 5.6 / 32.2 MiB | 9.14 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,000 / 1,000 | 33 msg/s | _pending_ | 118 / 193 ms | 32.8 / 5.5 MiB | 13.48 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 1,009 / 1,009 | 34 msg/s | _pending_ | 117 / 185 ms | 32.7 / 32.2 MiB | 30.25 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 842 / 842 | 28 msg/s | _pending_ | 144 / 199 ms | 5.4 / 5.5 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## request-response-longfast (v1)

Shallow-windowed RPC through a shaped pipe at Meshtastic LongFast timings (~1,070 bps airtime, ~175 ms preamble latency): command-and-telemetry exchanges on the wire mesh radios actually have. A 40-byte request and its 200-byte response cost ~2 seconds of channel time between them, so requests per minute and the protocol's byte tax are the products - window 2, because a real LoRa channel cannot pipeline deep without starving its own ACKs.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 27 / 27 | 0 msg/s | _pending_ | 4653 / 7511 ms | 5.2 / 31.7 MiB | 417.22 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 26 / 26 | 0 msg/s | _pending_ | 4669 / 7517 ms | 5.1 / 5.1 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 26 / 26 | 0 msg/s | _pending_ | 4436 / 7532 ms | 31.7 / 5.2 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 27 / 27 | 0 msg/s | _pending_ | 4669 / 7525 ms | 31.7 / 31.6 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-degraded-bluetooth (v1)

Sequential resource transfers (20-60 KB attachments) through a shaped pipe at a degraded-Bluetooth wire: 100 kbps serialization with 30 ms one-way latency. The first scenario where the rtt-proportional machinery - AIMD window growth, eifr-derived deadlines, part-retry schedules - operates above its loopback floors. Time-to-complete is the product; the pipe counts every wire byte, so payload-per-wire-byte is the protocol's measured overhead tax.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 9 / 9 | _pending_ | 12 kB/s | _pending_ | 7.6 / 6.5 MiB | _pending_ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 9 / 9 | _pending_ | 12 kB/s | _pending_ | 7.6 / 32.2 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 9 / 9 | _pending_ | 12 kB/s | _pending_ | 32.7 / 6.3 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 9 / 9 | _pending_ | 12 kB/s | _pending_ | 32.6 / 32.3 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-longfast (v1)

Sequential small resource transfers (1-3 KB, the LoRa-realistic band) through a shaped pipe at Meshtastic LongFast timings: SF11/250kHz airtime is ~1,070 bps effective serialization, and the 20-symbol preamble plus sync is ~175 ms of fixed per-hop latency. A 2 KB resource occupies the channel for ~15 seconds, so every handshake round trip the protocol spends is visible to the second. This is the deployment regime Reticulum is built for - the wire the dynamics machinery was designed against and the loopback suite can never reach.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 6.0 / 5.9 MiB | 8859.61 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 6.2 / 31.6 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 31.9 / 6.0 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 31.7 / 31.9 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## single-firehose (v2)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 299,269 / 299,269 | 10.0k msg/s | 2.2 MB/s | 1 / 2 ms | 7.9 / 20.2 MiB | 2.01 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 198,326 / 198,326 | 6.6k msg/s | 1.5 MB/s | 2 / 3 ms | 7.1 / 79.9 MiB | 3.88 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 62,861 / 62,861 | 2.1k msg/s | 461 kB/s | 0 / 3 ms | 40.9 / 8.4 MiB | 7.84 mJ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 63,690 / 63,690 | 2.1k msg/s | 468 kB/s | 0 / 0 ms | 41.7 / 24.5 MiB | 9.66 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 50,039 / 50,039 | 1.7k msg/s | 367 kB/s | 0 / 25 ms | 39.6 / 43.6 MiB | 12.44 mJ |
| go-reticulum → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 57,806 / 57,837 · 30 timed out | 1.7k msg/s | 364 kB/s | 0 / 5 ms | 20.2 / 21.7 MiB | 2.24 mJ |
| go-reticulum → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 215,748 / 215,755 | 6.2k msg/s | 1.4 MB/s | 0 / 2 ms | 29.4 / 16.3 MiB | 2.53 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 178,915 / 178,916 | 5.1k msg/s | 1.1 MB/s | 0 / 5 ms | 29.5 / 80.1 MiB | 3.34 mJ |
| Prns → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 29,009 / 29,057 · 32 timed out | 829 msg/s | 182 kB/s | 1 / 2 ms | 5.6 / 18.3 MiB | _pending_ |

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
