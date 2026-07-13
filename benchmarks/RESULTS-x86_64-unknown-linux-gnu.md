# Benchmark results — `x86_64-unknown-linux-gnu`

[← All hosts](RESULTS.md)

## Machine

- **CPU** — 12th Gen Intel(R) Core(TM) i7-1260P
- **Cores** — 12 physical / 16 logical
- **Memory** — 31.0 GiB
- **OS** — Linux (Ubuntu 22.04)
- **Kernel** — 6.8.0-124-generic

## channel-firehose-small-payload (v2)

One link's channel saturated with windowed small messages for a fixed wall-time - the same data shape as link-firehose, carried through Reticulum's Channel instead of the bare link: sequenced, msgtype-tagged delivery whose send window opens at the RTT tier, grows one step per proof toward a tiered ceiling, and shrinks one step per loss. Against link-firehose - the identical payload over the raw link - the contrast isolates what the Channel envelope and its adaptive window cost, and whether the window earns the throughput back.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 493,565 / 493,565 | 16.5k msg/s | 3.9 MB/s | 0 / 1 ms | 9.0 / 5.2 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)

## link-firehose-chain (v1)

The link firehose carried across a trunk of five pure transport nodes - six hops leaf to leaf, every packet switched five times each way. What it measures at the manifest's window: how latency stacks per hop and what that does to a fixed-window flow - the deployment truth for interactive apps. It is NOT a switch-cost meter: the trunk nodes loaf (~14us per switched packet) while Little's law (window / end-to-end rtt) sets the throughput. Probe pipeline capacity instead with a deeper-window manifest via the runner's MANIFEST override - and expect the protocol's own rtt*6 receipt deadline to expire receipts en masse if the window outruns capacity*6*rtt: deep windows are self-defeating by the link's design.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,500,091 / 1,500,091 | 50.0k msg/s | 12.0 MB/s | 0 / 1 ms | 17.0 / 43.8 MiB | _pending_ |

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)

## link-firehose-small-payload (v2)

One link saturated with windowed small single-packet sends for a fixed wall-time - the same data shape as single-firehose carried by the other mechanism: a session key amortizes the per-message crypto that singles pay per packet, while ProveAll still proves every delivery. The contrast between the two firehoses is the measurement.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1,531,462 / 1,531,462 | 51.0k msg/s | 12.3 MB/s | 0 / 1 ms | 17.2 / 43.7 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 170,822 / 170,838 · 16 timed out | 5.7k msg/s | 1.4 MB/s | 3 / 5 ms | 7.0 / 80.2 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 159,329 / 159,645 · 316 timed out | 5.3k msg/s | 1.3 MB/s | 1 / 2 ms | 85.5 / 11.1 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 154,737 / 154,866 · 129 timed out | 5.1k msg/s | 1.2 MB/s | 2 / 4 ms | 70.4 / 64.7 MiB | _pending_ |

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

## resource-bulk (v2)

A single large resource transferred whole, over and over, for a fixed wall-time - the multi-segment bulk mechanism. Each logical transfer is 64 MiB sliced into MAX_EFFICIENT_SIZE segments, sent one at a time and proved before the next, so the engine and the host each hold a single segment while the receiver appends the stream to disk-sized totals. Against resource-transfer (one segment) this measures whether the per-byte rate holds past the single-segment ceiling and whether peak memory stays flat at one segment regardless of total size. Compression is off on BOTH stacks (the reference harness has always passed auto_compress=False), so this row is the pure transport rate; the codec-engaged posture is resource-bulk-compressed.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 127 / 127 | 4 msg/s | 282.7 MB/s | 235 / 277 ms | 140.8 / 135.9 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 34 / 34 | 1 msg/s | 75.3 MB/s | 904 / 1008 ms | 14.9 / 435.8 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 24 / 24 | 1 msg/s | 51.9 MB/s | 1285 / 1801 ms | 999.4 / 363.9 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8 / 8 | 0 msg/s | 16.2 MB/s | 4251 / 4857 ms | 223.6 / 8.5 MiB | _pending_ |

> _The RNS 1.3.5 → Prns row is reference-sender-bound, not engine-bound. RNS prepares each segment lazily on a background thread and naps in 50 ms quanta (`Resource.py`: `while self.next_segment == None: time.sleep(0.05)`) while our receiver — which proves a segment in ~5 ms — waits, so the RNS sender sits idle ~80% of the run. The figure measures CPython's segment-prep pipelining, not the Prns receiver; the receiver's own rate is the Prns → Prns row._

**Implementations.**

- **Prns** — Rust, ed25519-dalek 2.2 · [https://github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- **RNS 1.3.5** — Python, PyCA cryptography / OpenSSL · [https://github.com/markqvist/Reticulum](https://github.com/markqvist/Reticulum) @ `1.3.5` · Reticulum License

## resource-bulk-compressed (v1)

resource-bulk with both stacks in their SHIPPING compression posture (RNS auto_compress=True, our SegmentCompression::AUTO) over the same incompressible payload - the codec-engaged bulk row. Dense data is the honest common bulk case (media, archives, encrypted blobs), so this measures what each sender PAYS to discover compression will not help: stock RNS runs the full level-9 bz2 attempt on every segment (~75 ms/MiB, an ~110 Mbit/s sender ceiling), our sender answers the same question with a 3x16 KiB head/mid/tail sample (~5 ms/MiB). The delta against resource-bulk is each stack's compression-attempt tax.

Each row is one live pairing — the initiator drives a windowed firehose at the responder over loopback, and every figure is the protocol's own: delivery proven by receipt, latency from the proofs, energy bracketed around the run. Conformant pairings rank first, ordered by energy per delivered message — a cheap-but-broken run never tops the table; energy needs `sudo` for the power counters and renders pending without it. Numbers compare within a host, never across.

| Initiator → Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |
|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 69 / 69 | 2 msg/s | 152.4 MB/s | 439 / 467 ms | 147.8 / 135.8 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 32 / 32 | 1 msg/s | 70.6 MB/s | 948 / 1271 ms | 22.3 / 447.9 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 5 / 5 | 0 msg/s | 9.0 MB/s | 7449 / 7517 ms | 270.3 / 119.0 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4 / 5 · 1 timed out | 0 msg/s | 6.8 MB/s | 7062 / 7072 ms | 303.6 / 8.6 MiB | _pending_ |

> _Every segment declines compression (the payload is deliberately incompressible), so the wire carries identical bytes to resource-bulk; the row isolates sender-side attempt cost. A compressible-payload row (keep path engaged) is a separate future scenario - it would measure codec throughput, not attempt tax._

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
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 797,683 / 797,683 | 26.6k msg/s | 5.8 MB/s | 0 / 2 ms | 10.8 / 35.6 MiB | _pending_ |
| Prns → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 174,960 / 174,960 | 5.8k msg/s | 1.3 MB/s | 3 / 4 ms | 5.9 / 80.3 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → RNS 1.3.5 _(ref)_ | <img src="assets/check.svg" width="14" alt="conformant" /> 62,732 / 62,732 | 2.1k msg/s | 460 kB/s | 0 / 1 ms | 41.9 / 46.3 MiB | _pending_ |
| RNS 1.3.5 _(ref)_ → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 59,030 / 59,030 | 2.0k msg/s | 433 kB/s | 0 / 1 ms | 41.5 / 7.6 MiB | _pending_ |

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
