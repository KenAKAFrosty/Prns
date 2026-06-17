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
| go-reticulum → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 16,393 / 16,888 · 495 timed out | 536 msg/s | 129 kB/s | 0 / 0 ms | 38.9 / 18.3 MiB | 0.30 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 722,437 / 722,437 | 24.1k msg/s | 5.8 MB/s | 0 / 1 ms | 11.1 / 41.7 MiB | 0.98 mJ |
| go-reticulum → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 325,264 / 325,736 · 472 timed out | 10.6k msg/s | 2.5 MB/s | 0 / 0 ms | 92.8 / 21.9 MiB | 1.99 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 201,710 / 202,315 · 605 timed out | 6.7k msg/s | 1.6 MB/s | 2 / 3 ms | 7.2 / 80.2 MiB | 2.63 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 209,812 / 209,825 · 13 timed out | 7.0k msg/s | 1.7 MB/s | 2 / 3 ms | 50.8 / 80.9 MiB | 3.24 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 177,077 / 177,078 · 1 timed out | 5.9k msg/s | 1.4 MB/s | 2 / 2 ms | 85.7 / 14.3 MiB | 3.39 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 170,266 / 170,267 · 1 timed out | 5.7k msg/s | 1.4 MB/s | 2 / 3 ms | 85.7 / 80.0 MiB | 4.38 mJ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 1,490 / 6,813,767 · 6,812,277 timed out | 50 msg/s | 12 kB/s | 1 / 1 ms | 32.3 / 17.5 MiB | 229.77 mJ |
| Prns → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 6,728 / 6,871 · 143 timed out | _pending_ | _pending_ | _pending_ | 5.5 / 18.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT

## request-response-degraded-bluetooth (v1)

Windowed RPC through a shaped pipe at a degraded-Bluetooth wire (100 kbps, 30 ms one-way): the interactive pattern under the constraint a phone-to-node hop actually has. Latency is now wire-dominated instead of engine-dominated, so the product is how little the protocol adds on top of the physics.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 960 / 960 | 32 msg/s | _pending_ | 124 / 164 ms | 5.4 / 5.5 MiB | 20.04 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 960 / 960 | 32 msg/s | _pending_ | 125 / 164 ms | 5.5 / 32.1 MiB | 30.55 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 958 / 958 | 32 msg/s | _pending_ | 124 / 162 ms | 32.5 / 5.4 MiB | 37.08 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 957 / 957 | 32 msg/s | _pending_ | 125 / 163 ms | 32.4 / 32.0 MiB | 54.22 mJ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## request-response-internet (v1)

Windowed RPC through a shaped pipe at an internet wire (50 Mbps, 25 ms one-way): remote-instance queries where the round trip dominates everything. Requests per second is window-bound physics; the protocol's job is to add nothing on top.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2,252 / 2,252 | 75 msg/s | _pending_ | 53 / 54 ms | 33.7 / 5.6 MiB | 9.68 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,254 / 2,254 | 75 msg/s | _pending_ | 53 / 55 ms | 5.7 / 32.7 MiB | 53.89 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,233 / 2,233 | 74 msg/s | _pending_ | 54 / 55 ms | 33.7 / 32.9 MiB | 70.64 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2,260 / 2,260 | 75 msg/s | _pending_ | 53 / 54 ms | 5.7 / 5.6 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## request-response-longfast (v1)

Shallow-windowed RPC through a shaped pipe at Meshtastic LongFast timings (~1,070 bps airtime, ~175 ms preamble latency): command-and-telemetry exchanges on the wire mesh radios actually have. A 40-byte request and its 200-byte response cost ~2 seconds of channel time between them, so requests per minute and the protocol's byte tax are the products - window 2, because a real LoRa channel cannot pipeline deep without starving its own ACKs.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 27 / 27 | 0 msg/s | _pending_ | 4651 / 6506 ms | 31.7 / 5.2 MiB | 484.17 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 27 / 27 | 0 msg/s | _pending_ | 4643 / 6544 ms | 5.1 / 31.5 MiB | 1519.32 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 27 / 27 | 0 msg/s | _pending_ | 4635 / 6536 ms | 5.2 / 5.1 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 27 / 27 | 0 msg/s | _pending_ | 4651 / 6567 ms | 31.8 / 31.8 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-bulk (v1)

A single large resource transferred whole, over and over, for a fixed wall-time - the multi-segment bulk mechanism. Each logical transfer is 64 MiB sliced into MAX_EFFICIENT_SIZE segments, sent one at a time and proved before the next, so the engine and the host each hold a single segment while the receiver appends the stream to disk-sized totals. Against resource-transfer (one segment) this measures whether the per-byte rate holds past the single-segment ceiling and whether peak memory stays flat at one segment regardless of total size.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 95 / 95 | 3 msg/s | 212.0 MB/s | 316 / 326 ms | 42.1 / 41.5 MiB | 4668.37 mJ · i 2642.41 / r 2025.95 |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 10 / 10 | 0 msg/s | 21.7 MB/s | 3114 / 3274 ms | 316.6 / 9.3 MiB | 11944.12 mJ · i 9597.95 / r 2346.17 |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 36 / 36 | 1 msg/s | 78.5 MB/s | 839 / 1106 ms | 10.0 / 472.9 MiB | 12433.47 mJ · i 2967.12 / r 9466.35 |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 30 / 30 | 1 msg/s | 66.0 MB/s | 917 / 1670 ms | 1066.5 / 429.0 MiB | 14688.32 mJ · i 7334.09 / r 7354.24 |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-degraded-bluetooth (v1)

Sequential resource transfers (20-60 KB attachments) through a shaped pipe at a degraded-Bluetooth wire: 100 kbps serialization with 30 ms one-way latency. The first scenario where the rtt-proportional machinery - AIMD window growth, eifr-derived deadlines, part-retry schedules - operates above its loopback floors. Time-to-complete is the product; the pipe counts every wire byte, so payload-per-wire-byte is the protocol's measured overhead tax.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | _pending_ | 11 kB/s | _pending_ | 7.6 / 32.1 MiB | 4.71 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | _pending_ | 11 kB/s | _pending_ | 32.6 / 32.2 MiB | 516.82 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | _pending_ | 11 kB/s | _pending_ | 7.6 / 6.3 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | _pending_ | 11 kB/s | _pending_ | 32.5 / 6.4 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-internet (v1)

Sequential resource transfers (20-120 KB) through a shaped pipe at an internet wire: 50 Mbps with 25 ms one-way latency - the TCP hop to a remote community instance. Bandwidth is cheap, round trips are not: every handshake the protocol spends costs 50 ms.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 110 / 110 | _pending_ | 255 kB/s | _pending_ | 34.3 / 6.8 MiB | 155.94 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 123 / 123 | _pending_ | 281 kB/s | _pending_ | 11.0 / 9.5 MiB | 546.20 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 113 / 113 | _pending_ | 262 kB/s | _pending_ | 8.1 / 36.0 MiB | 1714.69 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 110 / 110 | _pending_ | 255 kB/s | _pending_ | 34.4 / 35.9 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-local-wifi (v1)

Sequential resource transfers (20-120 KB) through a shaped pipe at a local-WiFi wire: 25 Mbps with 3 ms one-way latency - the hop between a phone and the node across the room. Fast enough that the engine is visible again, slow enough that the wire still meters it.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 545 / 545 | _pending_ | 1.2 MB/s | _pending_ | 38.3 / 42.7 MiB | 31.04 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 550 / 550 | _pending_ | 1.2 MB/s | _pending_ | 38.5 / 8.8 MiB | 91.56 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 608 / 608 | _pending_ | 1.4 MB/s | _pending_ | 8.1 / 43.4 MiB | 267.31 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 662 / 662 | _pending_ | 1.5 MB/s | _pending_ | 11.5 / 12.0 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-longfast (v1)

Sequential small resource transfers (1-3 KB, the LoRa-realistic band) through a shaped pipe at Meshtastic LongFast timings: SF11/250kHz airtime is ~1,070 bps effective serialization, and the 20-symbol preamble plus sync is ~175 ms of fixed per-hop latency. A 2 KB resource occupies the channel for ~15 seconds, so every handshake round trip the protocol spends is visible to the second. This is the deployment regime Reticulum is built for - the wire the dynamics machinery was designed against and the loopback suite can never reach.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 31.8 / 6.1 MiB | 25973.89 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 6.1 / 31.8 MiB | 35014.27 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 31.8 / 31.6 MiB | 66307.36 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 4 | _pending_ | 103 B/s | _pending_ | 6.1 / 6.0 MiB | _pending_ |

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

- _Conformance_ — every sent message accounted for, shown as `delivered / sent`. Extra suffixes call out messages that timed out or landed in a scenario-declared `raced` bucket, such as the RNS 1.3.1 request-response send-before-register loopback race.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules per delivered message. The power counters are package-domain, so this is the *combined* cost of both roles on the SoC; only the diagonal (a self-pair) is a single impl. The `i … / r …` split apportions it to initiator vs responder by their CPU-time share — the honest cross-platform proxy (Linux RAPL has no per-process counter), exact only insofar as power tracks CPU time. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env "PATH=$PATH" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
