# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — Apple M4
- **Cores** — 10 physical / 10 logical
- **Memory** — 16.0 GiB
- **OS** — macOS 26.4
- **Kernel** — 25.4.0

## channel-firehose-small-payload (v2)

One link's channel saturated with windowed small messages for a fixed wall-time - the same data shape as link-firehose, carried through Reticulum's Channel instead of the bare link: sequenced, msgtype-tagged delivery whose send window opens at the RTT tier, grows one step per proof toward a tiered ceiling, and shrinks one step per loss. Against link-firehose - the identical payload over the raw link - the contrast isolates what the Channel envelope and its adaptive window cost, and whether the window earns the throughput back.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 966,965 / 966,965 | 32.2k msg/s | 7.7 MB/s | 0 / 1 ms | 20.8 / 11.1 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)

## link-firehose-small-payload (v2)

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 689,919 / 690,005 · 86 timed out | 23.0k msg/s | 5.5 MB/s | 1 / 1 ms | 15.7 / 15.0 MiB | 0.31 mJ |
| go-reticulum → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 346,559 / 347,029 · 470 timed out | 11.3k msg/s | 2.7 MB/s | 0 / 0 ms | 95.0 / 25.5 MiB | 0.55 mJ |
| LXMF-rs 0.2.0 → LXMF-rs 0.2.0 | <img src="assets/check.svg" width="14" alt="conformant" /> 16,401 / 16,401 | 468 msg/s | 112 kB/s | 0 / 0 ms | 17.6 / 17.6 MiB | 0.58 mJ |
| go-reticulum → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 345,325 / 345,792 · 467 timed out | 11.3k msg/s | 2.7 MB/s | 0 / 0 ms | 95.2 / 14.8 MiB | 0.60 mJ |
| go-reticulum → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 5,719 / 6,215 · 496 timed out | 187 msg/s | 45 kB/s | 0 / 0 ms | 27.3 / 21.6 MiB | 0.70 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 251,415 / 251,416 · 1 timed out | 8.4k msg/s | 2.0 MB/s | 2 / 2 ms | 10.3 / 110.9 MiB | 1.11 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 246,072 / 246,072 | 8.2k msg/s | 2.0 MB/s | 1 / 2 ms | 73.8 / 110.0 MiB | 1.15 mJ |
| RNS 1.3.1 _(ref)_ → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 153,620 / 153,984 · 364 timed out | 5.1k msg/s | 1.2 MB/s | 1 / 1 ms | 87.2 / 14.8 MiB | 1.37 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 130,302 / 130,696 · 394 timed out | 4.3k msg/s | 1.0 MB/s | 0 / 1 ms | 82.6 / 14.4 MiB | 1.39 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 149,858 / 150,201 · 343 timed out | 4.9k msg/s | 1.2 MB/s | 1 / 2 ms | 86.0 / 79.2 MiB | 2.01 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,055,241 / 1,055,257 · 16 timed out | 35.2k msg/s | 8.4 MB/s | 0 / 1 ms | 21.6 / 68.5 MiB | _pending_ |
| Prns → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 6,590 / 6,733 · 143 timed out | _pending_ | _pending_ | _pending_ | 7.7 / 21.9 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 5,819 / 5,990 · 171 timed out | _pending_ | _pending_ | _pending_ | 43.5 / 21.5 MiB | _pending_ |
| Leviculum 0.6.3 → Leviculum 0.6.3 | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 14,160 / 14,176 | 674 msg/s | 162 kB/s | 17 / 24 ms | 9.8 / 9.6 MiB | 0.11 mJ |
| Leviculum 0.6.3 → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 0 / 16 | 0 msg/s | 0 B/s | 0 / 0 ms | 8.1 / 5.7 MiB | _pending_ |
| Leviculum 0.6.3 → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 0 / 16 | 0 msg/s | 0 B/s | 0 / 0 ms | 8.2 / 41.7 MiB | _pending_ |
| Leviculum 0.6.3 → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 0 / 16 | 0 msg/s | 0 B/s | 0 / 0 ms | 8.2 / 15.6 MiB | _pending_ |

**Implementations.**

- **LXMF-rs 0.2.0** — Rust, ed25519-dalek 2.1 · [https://github.com/FreeTAKTeam/LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs) @ `30da190` · EPL-2.0
  - _Link-firehose only, and a self-pair ceiling. LXMF-rs fields no single node (single-packet proofs are unimplemented), and its link wire — plain link data — interoperates only with itself (one-directionally with Prns). This surfaces a real dynamic: 'link' is not one protocol across the family — go proves single-style link packets, Leviculum carries a Channel multiplexer, Prns/LXMF-rs exchange plain link data — so cross-impl link pairings only work where those sub-protocols line up. Delivery on the receiver is counted directly; the reliable in-order link gives the sender delivered==sent with no per-message RTT._
- **Leviculum 0.6.3** — Rust, ed25519-dalek 2.2 · [https://codeberg.org/Lew_Palm/leviculum](https://codeberg.org/Lew_Palm/leviculum) @ `6f366ca` · AGPL-3.0-or-later
- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT

## request-response (v1)

Windowed RPC round trips over one link - the network's most common interactive pattern (page fetches, queries, telemetry): each request a varied size, each naming a varied response size it wants back, the handler answering through the engine-gated allow list. Latency is the product; requests per second the capacity.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2,029,245 / 2,029,245 | 67.6k msg/s | _pending_ | 0 / 1 ms | 84.2 / 63.3 MiB | 0.11 mJ · i 0.05 / r 0.06 |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 311,553 / 311,811 · 258 raced | 10.4k msg/s | _pending_ | 0 / 0 ms | 151.6 / 36.8 MiB | 0.72 mJ · i 0.61 / r 0.12 |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 179,918 / 179,922 · 4 timed out | 6.0k msg/s | _pending_ | 0 / 1 ms | 24.6 / 221.7 MiB | 1.02 mJ · i 0.14 / r 0.88 |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 258,939 / 258,941 · 2 raced | 8.6k msg/s | _pending_ | 0 / 1 ms | 135.2 / 116.0 MiB | 1.15 mJ · i 0.56 / r 0.59 |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## request-response-cellular-fast-4g (v1)

Windowed RPC through a shaped pipe at a fast-4G cellular wire (10 Mbps, 60 ms one-way): the interactive pattern on a plausible mobile hop where round-trip delay dominates throughput. Requests per second is window-bound physics; the protocol's job is to add little on top.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 952 / 952 | 32 msg/s | _pending_ | 126 / 128 ms | 10.3 / 42.1 MiB | 4.60 mJ · i 0.36 / r 4.23 |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 956 / 956 | 32 msg/s | _pending_ | 126 / 127 ms | 10.3 / 10.3 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 952 / 952 | 32 msg/s | _pending_ | 126 / 128 ms | 42.8 / 10.2 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 948 / 948 | 32 msg/s | _pending_ | 126 / 129 ms | 42.5 / 42.4 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-local-wifi (v1)

Sequential resource transfers (20-120 KB) through a shaped pipe at a local-WiFi wire: 25 Mbps with 3 ms one-way latency - the hop between a phone and the node across the room. Fast enough that the engine is visible again, slow enough that the wire still meters it.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 708 / 708 | 24 msg/s | 1.6 MB/s | 42 / 60 ms | 12.4 / 11.4 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 654 / 654 | 22 msg/s | 1.5 MB/s | 43 / 70 ms | 50.8 / 8.0 MiB | _pending_ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 649 / 649 | 22 msg/s | 1.4 MB/s | 42 / 70 ms | 8.4 / 45.3 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 632 / 632 | 21 msg/s | 1.4 MB/s | 44 / 73 ms | 50.5 / 45.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-local-wifi-bulk (v1)

Sequential maximum-size resource transfers through a shaped pipe at a local-WiFi wire: 25 Mbps with 3 ms one-way latency. This is the high-speed bulk-resource canary: one large sealed stream sliced into parts, pulled by the receiver inside its AIMD window, and hash-proved whole before the next resource starts.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 82 / 82 | 3 msg/s | 2.9 MB/s | 366 / 369 ms | 25.7 / 24.2 MiB | _pending_ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 76 / 76 | 3 msg/s | 2.6 MB/s | 391 / 440 ms | 9.7 / 53.8 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 75 / 75 | 2 msg/s | 2.6 MB/s | 394 / 441 ms | 59.8 / 8.7 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 75 / 75 | 2 msg/s | 2.6 MB/s | 397 / 450 ms | 59.5 / 53.8 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-transfer (v1)

Sequential maximum-size resource transfers over one link for a fixed wall-time - the bulk mechanism: one sealed stream sliced into parts, pulled by the receiver inside its AIMD window, proved whole by hash. Goodput counts settled transfers' payload bytes; the protocol itself hash-verifies every byte that arrives.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 9,160 / 9,160 | 305 msg/s | 320.1 MB/s | 3 / 3 ms | 28.4 / 28.2 MiB | 12.06 mJ · i 6.79 / r 5.27 |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 5,500 / 5,500 | 183 msg/s | 192.2 MB/s | 5 / 6 ms | 614.4 / 12.1 MiB | 30.41 mJ · i 21.59 / r 8.82 |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 3,312 / 3,312 | 110 msg/s | 115.7 MB/s | 9 / 9 ms | 12.3 / 147.0 MiB | 44.73 mJ · i 11.08 / r 33.65 |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,854 / 2,854 | 95 msg/s | 99.8 MB/s | 10 / 12 ms | 354.9 / 137.7 MiB | 54.73 mJ · i 20.47 / r 34.26 |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## single-firehose (v2)

A ProveAll destination saturated with windowed SINGLE packets for a fixed wall-time: sustained one-shot message throughput, goodput, and settlement latency from the protocol's own proofs. No link - singles are the protocol's native shape for high-volume small one-shots, each proof carrying the RTT.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| rns-cr 0.1.0 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 7,972 / 7,972 | 266 msg/s | 59 kB/s | 1 / 3 ms | 31.0 / 8.0 MiB | 0.24 mJ |
| Prns → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 377,951 / 377,951 | 12.6k msg/s | 2.8 MB/s | 1 / 2 ms | 13.3 / 14.8 MiB | 0.60 mJ |
| go-reticulum → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 499,205 / 499,205 | 16.6k msg/s | 3.7 MB/s | 0 / 0 ms | 65.6 / 14.9 MiB | 0.75 mJ |
| go-reticulum → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 305,407 / 305,407 | 10.2k msg/s | 2.2 MB/s | 0 / 0 ms | 45.2 / 23.3 MiB | 0.86 mJ |
| Leviculum 0.6.3 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 97,073 / 97,073 | 3.2k msg/s | 712 kB/s | 4 / 11 ms | 31.7 / 12.6 MiB | 1.60 mJ |
| Leviculum 0.6.3 → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 95,311 / 95,311 | 3.2k msg/s | 699 kB/s | 4 / 11 ms | 31.4 / 14.6 MiB | 1.68 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 123,813 / 123,813 | 4.1k msg/s | 908 kB/s | 4 / 4 ms | 9.3 / 75.2 MiB | 1.93 mJ |
| rns-cr 0.1.0 → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 7,673 / 7,673 | 256 msg/s | 56 kB/s | 1 / 4 ms | 30.5 / 9.4 MiB | 1.98 mJ |
| rns-cr 0.1.0 → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 6,489 / 6,489 | 216 msg/s | 48 kB/s | 1 / 4 ms | 30.5 / 43.3 MiB | 2.25 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 124,984 / 124,984 | 4.2k msg/s | 916 kB/s | 1 / 1 ms | 30.9 / 75.6 MiB | 2.51 mJ |
| Leviculum 0.6.3 → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 90,653 / 90,653 | 3.0k msg/s | 664 kB/s | 3 / 11 ms | 30.6 / 70.3 MiB | 2.52 mJ |
| RNS 1.3.1 _(ref)_ → Leviculum 0.6.3 | <img src="assets/check.svg" width="14" alt="conformant" /> 75,240 / 75,240 | 2.5k msg/s | 552 kB/s | 3 / 4 ms | 53.2 / 12.6 MiB | 2.74 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 32,188 / 32,188 | 1.1k msg/s | 236 kB/s | 5 / 25 ms | 47.5 / 9.2 MiB | 7.22 mJ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 25,429 / 25,429 | 847 msg/s | 186 kB/s | 11 / 28 ms | 47.1 / 22.2 MiB | 8.58 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 28,327 / 28,327 | 944 msg/s | 208 kB/s | 8 / 26 ms | 47.1 / 48.5 MiB | 9.22 mJ |
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 69,958 / 69,958 | 14.0k msg/s | 3.1 MB/s | 1 / 2 ms | 9.5 / 8.9 MiB | _pending_ |
| rns-cr 0.1.0 → go-reticulum | <img src="assets/check.svg" width="14" alt="conformant" /> 7,489 / 7,489 | 250 msg/s | 55 kB/s | 1 / 4 ms | 30.6 / 21.3 MiB | _pending_ |
| go-reticulum → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 26,496 / 26,544 · 32 timed out | 757 msg/s | 166 kB/s | 0 / 0 ms | 22.8 / 22.5 MiB | 0.08 mJ |
| Prns → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 71,783 / 71,831 · 32 timed out | 2.1k msg/s | 451 kB/s | 1 / 1 ms | 9.0 / 28.5 MiB | 0.57 mJ |
| Leviculum 0.6.3 → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 19,985 / 20,033 · 32 timed out | 571 msg/s | 126 kB/s | 1 / 2 ms | 16.0 / 21.9 MiB | 1.15 mJ |

**Implementations.**

- **Leviculum 0.6.3** — Rust, ed25519-dalek 2.2 · [https://codeberg.org/Lew_Palm/leviculum](https://codeberg.org/Lew_Palm/leviculum) @ `6f366ca` · AGPL-3.0-or-later
- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT
- **rns-cr 0.1.0** — Crystal, OpenSSL EVP (spider-gazelle/ed25519) · [https://github.com/jtippett/rns-cr](https://github.com/jtippett/rns-cr) @ `514c309` · MIT
  - _Single-firehose initiator only — at this commit rns-cr does not prove incoming single packets and never resolves a link data-packet proof, so it fields no responder or link node and appears only as a single initiator._

---

- _Conformance_ — every sent message accounted for, shown as `delivered / sent`. Extra suffixes call out messages that timed out or landed in a scenario-declared `raced` bucket, such as the RNS 1.3.1 request-response send-before-register loopback race.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules per delivered message. The power counters are package-domain, so this is the *combined* cost of both roles on the SoC; only the diagonal (a self-pair) is a single impl. The `i … / r …` split apportions it to initiator vs responder by their CPU-time share — the honest cross-platform proxy (Linux RAPL has no per-process counter), exact only insofar as power tracks CPU time. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env "PATH=$PATH" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
