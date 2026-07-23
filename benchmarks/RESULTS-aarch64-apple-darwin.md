# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

> **Qualification: INCOMPLETE.** 20/34 cells; 60/102 conformant samples; exact source `7c94f0923f5ba4af9828cb7705136c36af81510a`; source tree clean.

## Machine and method

Apple M4; 10 physical / 10 logical; 16.0 GiB; macOS 26.4.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Default-policy rows preserve each implementation's normal TCP bitrate and MTU policy. The controlled 1 Gbps resource rows change only that interface policy, both for real endpoint transfers and for transported-resource switching; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 38.9k/s | 1.6k/s | 24.46× |
| Link-message throughput | 75.2k/s | 4.7k/s | 15.95× |
| Request/response | 5.2k/s | 1.0k/s | 4.99× |
| Maximum resource segment | 269.15 MB/s | 95.64 MB/s | 2.81× |
| Maximum resource segment · 1 Gbps policy | — | — | — |
| 64 MiB resource stream | 460.08 MB/s | 120.02 MB/s | 3.83× |
| 64 MiB resource stream · 1 Gbps policy | — | — | — |
| Raw transport throughput | — | — | — |
| Transported resource throughput | — | — | — |
| Transported resource throughput · 1 Gbps policy | — | — | — |

A dash means no current three-sample release evidence is published for that scenario.

## Detailed results

### Links

#### Link-message throughput (v6)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 6745682/6745682 · 3/3 samples | 75.2k/s | 18.04 MB/s | <1.00 / 1.00 ms | i 24.7 MiB / r 48.1 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 776290/776290 · 3/3 samples | 8.6k/s | 2.06 MB/s | 2.00 / 2.00 ms | i 9.4 MiB / r 262.4 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 431040/431040 · 3/3 samples | 4.7k/s | 1.13 MB/s | 1.00 / 2.00 ms | i 234.6 MiB / r 231.8 MiB |
| RNS 1.4.0 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 340031/340031 · 3/3 samples | 3.8k/s | 902.6 kB/s | <1.00 / 1.00 ms | i 228.7 MiB / r 11.1 MiB |

**Cell context**

1. **RNS 1.4.0 (compiled) → Prns** — Published macOS and Windows suites both show this direction below the RNS↔RNS diagonal. It combines RNS's sender/receipt loop with Prns's link-data receive/proof-return path; the opposite mixed row reverses both roles, so mixed cells are compositions rather than interpolations. The cross-host repeat localizes the effect to this directional interop seam, but the current evidence does not assign the cost to one endpoint.

### Packets

#### Single-packet throughput (v4)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3506029/3506029 · 3/3 samples | 38.9k/s | 8.56 MB/s | <1.00 / 1.00 ms | i 16.1 MiB / r 48.5 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 369725/369725 · 3/3 samples | 4.1k/s | 907.7 kB/s | 4.00 / 4.00 ms | i 5.1 MiB / r 228.1 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 139520/139520 · 3/3 samples | 1.7k/s | 372.7 kB/s | <1.00 / 20.00 ms | i 201.9 MiB / r 7.2 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 142358/142358 · 3/3 samples | 1.6k/s | 350.6 kB/s | 1.00 / 21.00 ms | i 199.4 MiB / r 206.2 MiB |

### Requests

#### Request/response (v11)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 510260/510260 · 3/3 samples | 5.2k/s | 0.65 / 0.92 ms | i 18.1 MiB / r 46.4 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 189353/189353 · 3/3 samples | 2.1k/s | 1.61 / 4.09 ms | i 332.3 MiB / r 37.4 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 94154/94154 · 3/3 samples | 1.0k/s | 1.46 / 16.33 ms | i 284.2 MiB / r 341.8 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 90438/90438 · 3/3 samples | 1.0k/s | 1.13 / 251.44 ms | i 7.0 MiB / r 331.2 MiB |

### Resources

#### 64 MiB resource stream (v8)

Stream a 64 MiB incompressible resource with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 618/618 · 3/3 samples | 7/s | 460.08 MB/s | 145.00 / 154.00 ms | i 48.2 MiB / r 40.0 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 160/160 · 3/3 samples | 2/s | 122.62 MB/s | 494.00 / 815.00 ms | i 17.2 MiB / r 611.3 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 160/160 · 3/3 samples | 2/s | 120.02 MB/s | 547.00 / 581.00 ms | i 409.0 MiB / r 398.9 MiB |
| RNS 1.4.0 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.25/s | 16.63 MB/s | 4061.00 / 4332.00 ms | i 342.5 MiB / r 9.2 MiB |

**Cell context**

1. **RNS 1.4.0 (compiled) → Prns** — The original published macOS and Windows suites both settled this cell near 16.6 MB/s, while its one-segment control beat RNS↔RNS. RNS 1.4.0 splits 64 MiB into 65 protocol segments, prepares each successor in a background thread, and polls successor readiness in 50 ms increments when a proof arrives first. Prns's fast one-segment receiver repeatedly exposes that stock-sender handoff; this is a multi-segment pipeline interaction, not evidence that Prns is slow at receiving a segment.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 23173/23173 · 3/3 samples | 257/s | 269.15 MB/s | 3.00 / 4.00 ms | i 41.7 MiB / r 40.0 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15217/15217 · 3/3 samples | 169/s | 177.63 MB/s | 6.00 / 6.00 ms | i 717.0 MiB / r 9.9 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 9391/9391 · 3/3 samples | 104/s | 109.30 MB/s | 9.00 / 10.00 ms | i 10.9 MiB / r 294.0 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 8199/8199 · 3/3 samples | 91/s | 95.64 MB/s | 11.00 / 12.00 ms | i 493.2 MiB / r 286.6 MiB |

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.0 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, HDLC-framed TCP wire rates, relay-only CPU/RSS, and direct-driver headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
