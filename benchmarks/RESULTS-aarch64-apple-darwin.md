# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — Apple M4
- **Cores** — 10 physical / 10 logical
- **Memory** — 16.0 GiB
- **OS** — macOS 26.4
- **Kernel** — 25.4.0

## link-firehose-small-payload (v2)

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 786,111 / 786,111 | 26.2k msg/s | 6.3 MB/s | 0 / 1 ms | 15.7 / 48.2 MiB | 0.28 mJ |
| LXMF-rs 0.2.0 → LXMF-rs 0.2.0 | <img src="assets/check.svg" width="14" alt="conformant" /> 16,401 / 16,401 | 468 msg/s | 112 kB/s | 0 / 0 ms | 17.6 / 17.6 MiB | 0.58 mJ |
| go-reticulum → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 246,072 / 246,072 | 8.2k msg/s | 2.0 MB/s | 1 / 2 ms | 73.8 / 110.0 MiB | 1.15 mJ |
| Leviculum 0.6.3 → Leviculum 0.6.3 | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 14,160 / 14,176 | 674 msg/s | 162 kB/s | 17 / 24 ms | 9.8 / 9.6 MiB | 0.11 mJ |
| Prns → Leviculum 0.6.3 | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 689,919 / 690,005 · 86 timed out | 23.0k msg/s | 5.5 MB/s | 1 / 1 ms | 15.7 / 15.0 MiB | 0.31 mJ |
| go-reticulum → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 346,559 / 347,029 · 470 timed out | 11.3k msg/s | 2.7 MB/s | 0 / 0 ms | 95.0 / 25.5 MiB | 0.55 mJ |
| go-reticulum → Leviculum 0.6.3 | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 345,325 / 345,792 · 467 timed out | 11.3k msg/s | 2.7 MB/s | 0 / 0 ms | 95.2 / 14.8 MiB | 0.60 mJ |
| go-reticulum → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 5,719 / 6,215 · 496 timed out | 187 msg/s | 45 kB/s | 0 / 0 ms | 27.3 / 21.6 MiB | 0.70 mJ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 251,415 / 251,416 · 1 timed out | 8.4k msg/s | 2.0 MB/s | 2 / 2 ms | 10.3 / 110.9 MiB | 1.11 mJ |
| RNS 1.3.1 _(ref)_ → Leviculum 0.6.3 | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 153,620 / 153,984 · 364 timed out | 5.1k msg/s | 1.2 MB/s | 1 / 1 ms | 87.2 / 14.8 MiB | 1.37 mJ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 130,302 / 130,696 · 394 timed out | 4.3k msg/s | 1.0 MB/s | 0 / 1 ms | 82.6 / 14.4 MiB | 1.39 mJ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 149,858 / 150,201 · 343 timed out | 4.9k msg/s | 1.2 MB/s | 1 / 2 ms | 86.0 / 79.2 MiB | 2.01 mJ |
| Leviculum 0.6.3 → Prns | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 0 / 16 | 0 msg/s | 0 B/s | 0 / 0 ms | 8.1 / 5.7 MiB | _pending_ |
| Leviculum 0.6.3 → RNS 1.3.1 _(ref)_ | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 0 / 16 | 0 msg/s | 0 B/s | 0 / 0 ms | 8.2 / 41.7 MiB | _pending_ |
| Leviculum 0.6.3 → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 0 / 16 | 0 msg/s | 0 B/s | 0 / 0 ms | 8.2 / 15.6 MiB | _pending_ |
| Prns → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 6,590 / 6,733 · 143 timed out | _pending_ | _pending_ | _pending_ | 7.7 / 21.9 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → go-reticulum | <img src="assets/cross.svg" width="14" alt="non-conformant" /> 5,819 / 5,990 · 171 timed out | _pending_ | _pending_ | _pending_ | 43.5 / 21.5 MiB | _pending_ |

**Implementations.**

- **LXMF-rs 0.2.0** — Rust, ed25519-dalek 2.1 · [https://github.com/FreeTAKTeam/LXMF-rs](https://github.com/FreeTAKTeam/LXMF-rs) @ `30da190` · EPL-2.0
  - _Link-firehose only, and a self-pair ceiling. LXMF-rs fields no single node (single-packet proofs are unimplemented), and its link wire — plain link data — interoperates only with itself (one-directionally with Prns). This surfaces a real dynamic: 'link' is not one protocol across the family — go proves single-style link packets, Leviculum carries a Channel multiplexer, Prns/LXMF-rs exchange plain link data — so cross-impl link pairings only work where those sub-protocols line up. Delivery on the receiver is counted directly; the reliable in-order link gives the sender delivered==sent with no per-message RTT._
- **Leviculum 0.6.3** — Rust, ed25519-dalek 2.2 · [https://codeberg.org/Lew_Palm/leviculum](https://codeberg.org/Lew_Palm/leviculum) @ `6f366ca` · AGPL-3.0-or-later
- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License
- **go-reticulum** — Go, Go stdlib crypto/ed25519 · [https://github.com/svanichkin/go-reticulum](https://github.com/svanichkin/go-reticulum) @ `06621cc` · MIT

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
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 82 / 82 | 3 msg/s | 2.9 MB/s | 365 / 369 ms | 25.7 / 24.1 MiB | _pending_ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 77 / 77 | 3 msg/s | 2.7 MB/s | 389 / 437 ms | 9.6 / 53.9 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 76 / 76 | 3 msg/s | 2.6 MB/s | 392 / 436 ms | 59.8 / 8.6 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 76 / 76 | 3 msg/s | 2.6 MB/s | 394 / 445 ms | 59.4 / 53.8 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.1** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.1` · Reticulum License

## resource-transfer (v1)

Sequential maximum-size resource transfers over one link for a fixed wall-time - the bulk mechanism: one sealed stream sliced into parts, pulled by the receiver inside its AIMD window, proved whole by hash. Goodput counts settled transfers' payload bytes; the protocol itself hash-verifies every byte that arrives.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4,238 / 4,238 | 141 msg/s | 148.1 MB/s | 7 / 9 ms | 25.8 / 24.6 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3,611 / 3,611 | 120 msg/s | 126.2 MB/s | 8 / 10 ms | 429.1 / 9.2 MiB | _pending_ |
| Prns → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 3,159 / 3,159 | 105 msg/s | 110.4 MB/s | 9 / 10 ms | 9.7 / 141.2 MiB | _pending_ |
| RNS 1.3.1 _(ref)_ → RNS 1.3.1 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 2,834 / 2,834 | 94 msg/s | 99.0 MB/s | 11 / 12 ms | 346.1 / 133.8 MiB | _pending_ |

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

- _Conformance_ — settled clean: every sent message proved within the link's traffic timeout, shown as `delivered / sent`. A ✗ flags messages that timed out — a responder slower than `rtt × 6` misses the deadline by spec, not by fault.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules per delivered message. The power counters are package-domain, so this is the *combined* cost of both roles on the SoC; only the diagonal (a self-pair) is a single impl. The `i … / r …` split apportions it to initiator vs responder by their CPU-time share — the honest cross-platform proxy (Linux RAPL has no per-process counter), exact only insofar as power tracks CPU time. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env "PATH=$PATH" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
