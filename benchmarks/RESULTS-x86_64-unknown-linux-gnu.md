# Benchmark results — `x86_64-unknown-linux-gnu`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `ab1c9ef3e8b1d078c69ee170da2cd4e381760bf8`; source tree clean.

## Machine and method

12th Gen Intel(R) Core(TM) i7-1260P; 12 physical / 16 logical; 31.0 GiB; Linux (Ubuntu 22.04).

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Default-policy rows preserve each implementation's normal TCP bitrate and MTU policy. The controlled 1 Gbps resource rows change only that interface policy, both for real endpoint transfers and for transported-resource switching; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-x86_64-unknown-linux-gnu-dark.svg">
  <img alt="Bar chart of Prns median throughput as a multiple of RNS 1.4.2 (compiled) for each published scenario" src="assets/at-a-glance-x86_64-unknown-linux-gnu-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-memory-x86_64-unknown-linux-gnu-dark.svg">
  <img alt="Bar chart of RNS 1.4.2 (compiled) peak memory as a multiple of Prns for each role and scenario" src="assets/at-a-glance-memory-x86_64-unknown-linux-gnu-light.svg">
</picture>

<details>
<summary>Chart data as a table</summary>

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 25.3k/s | 3.5k/s | 7.19× |
| Link-message throughput | 50.8k/s | 3.8k/s | 13.40× |
| Request/response | 987/s | 569/s | 1.73× |
| Maximum resource segment | 186.30 MB/s | 55.64 MB/s | 3.35× |
| Maximum resource segment · 1 Gbps policy | 186.94 MB/s | 56.04 MB/s | 3.34× |
| 64-segment resource stream | 289.56 MB/s | 50.24 MB/s | 5.76× |
| 64-segment resource stream · 1 Gbps policy | 288.72 MB/s | 38.24 MB/s | 7.55× |
| Raw transport throughput | 167.05 MB/s | 8.09 MB/s | 20.65× |
| Transported resource throughput | 777.74 MB/s | 160.31 MB/s | 4.85× |
| Transported resource throughput · 1 Gbps policy | 691.07 MB/s | 146.45 MB/s | 4.72× |

| Scenario · role | Prns peak RSS | Reference peak RSS | Reference / Prns |
|---|---:|---:|---:|
| Single-packet throughput · initiator | 5.5 MiB | 207.7 MiB | 37.76× |
| Single-packet throughput · responder | 34.8 MiB | 214.0 MiB | 6.16× |
| Link-message throughput · initiator | 5.9 MiB | 222.5 MiB | 37.88× |
| Link-message throughput · responder | 43.9 MiB | 216.9 MiB | 4.94× |
| Request/response · initiator | 7.6 MiB | 239.8 MiB | 31.45× |
| Request/response · responder | 36.0 MiB | 270.4 MiB | 7.52× |
| Maximum resource segment · initiator | 42.4 MiB | 345.6 MiB | 8.15× |
| Maximum resource segment · responder | 39.6 MiB | 363.8 MiB | 9.18× |
| Maximum resource segment · 1 Gbps policy · initiator | 74.1 MiB | 320.8 MiB | 4.33× |
| Maximum resource segment · 1 Gbps policy · responder | 71.8 MiB | 614.3 MiB | 8.56× |
| 64-segment resource stream · initiator | 44.8 MiB | 1025.6 MiB | 22.88× |
| 64-segment resource stream · responder | 39.5 MiB | 475.4 MiB | 12.03× |
| 64-segment resource stream · 1 Gbps policy · initiator | 77.2 MiB | 632.4 MiB | 8.19× |
| 64-segment resource stream · 1 Gbps policy · responder | 71.5 MiB | 659.4 MiB | 9.22× |
| Raw transport throughput · relay | 44.4 MiB | 323.4 MiB | 7.29× |
| Transported resource throughput · relay | 140.9 MiB | 257.5 MiB | 1.83× |
| Transported resource throughput · 1 Gbps policy · relay | 525.1 MiB | 199.5 MiB | 0.38× |

A dash means no current three-sample release evidence is published for that scenario.

</details>

## Detailed results

### Links

#### Link-message throughput (v8)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4581906/4581906 · 3/3 samples | 50.8k/s | 12.20 MB/s | <1.00 / 1.00 ms | i 5.9 MiB / r 43.9 MiB | i 0.37 mJ / r 0.08 mJ |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 728043/728043 · 3/3 samples | 8.1k/s | 1.94 MB/s | 2.00 / 3.00 ms | i 5.6 MiB / r 242.7 MiB | i 1.50 mJ / r 1.02 mJ |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 504340/504340 · 3/3 samples | 5.7k/s | 1.37 MB/s | <1.00 / <1.00 ms | i 243.4 MiB / r 12.6 MiB | i 2.22 mJ / r 0.45 mJ |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 350550/350550 · 3/3 samples | 3.8k/s | 910.5 kB/s | <1.00 / 1.00 ms | i 222.5 MiB / r 216.9 MiB | i 2.38 mJ / r 2.16 mJ |

### Packets

#### Single-packet throughput (v6)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2280212/2280212 · 3/3 samples | 25.3k/s | 5.56 MB/s | <1.00 / 1.00 ms | i 5.5 MiB / r 34.8 MiB | i 0.62 mJ / r 0.46 mJ |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 824976/824976 · 3/3 samples | 9.4k/s | 2.07 MB/s | 2.00 / 2.00 ms | i 5.5 MiB / r 250.9 MiB | i 1.79 mJ / r 0.75 mJ |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 314870/314870 · 3/3 samples | 3.5k/s | 772.3 kB/s | 1.00 / 4.00 ms | i 207.7 MiB / r 214.0 MiB | i 3.68 mJ / r 1.52 mJ |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 267781/267781 · 3/3 samples | 3.0k/s | 654.6 kB/s | 1.00 / 5.00 ms | i 205.5 MiB / r 8.9 MiB | i 3.56 mJ / r 3.33 mJ |

### Requests

#### Request/response (v12)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 88652/88652 · 3/3 samples | 987/s | 3.89 / 5.63 ms | i 7.6 MiB / r 36.0 MiB | i 0.97 mJ / r 25.11 mJ |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 56294/56294 · 3/3 samples | 624/s | 6.15 / 8.80 ms | i 240.1 MiB / r 34.9 MiB | i 11.77 mJ / r 30.34 mJ |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 51215/51215 · 3/3 samples | 569/s | 4.38 / 12.87 ms | i 239.8 MiB / r 270.4 MiB | i 10.80 mJ / r 18.07 mJ |
| Prns → RNS 1.4.2 (compiled)<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 41192/41192 · 3/3 samples | 453/s | 2.16 / 254.03 ms | i 6.4 MiB / r 251.7 MiB | i 4.20 mJ / r 22.19 mJ |

**Cell context**

1. **Prns → RNS 1.4.2 (compiled)** — RNS sends a resource advertisement before registering that resource internally. Prns can return the first pull so quickly that RNS drops it. Prns then waits for its 250 ms retry deadline. A p99 pinned just above 250 ms is the fingerprint of this race.

### Resources

#### 64-segment resource stream (v10)

Stream 64 maximum-efficient resource segments with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 394/394 · 3/3 samples | 4/s | 289.56 MB/s | 231.00 / 245.00 ms | i 44.8 MiB / r 39.5 MiB | i 42.07 mJ/MiB / r 36.61 mJ/MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 118/118 · 3/3 samples | 1/s | 86.09 MB/s | 740.00 / 937.00 ms | i 14.0 MiB / r 592.6 MiB | i 51.04 mJ/MiB / r 163.78 mJ/MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 69/69 · 3/3 samples | 0.75/s | 50.24 MB/s | 1268.00 / 1839.00 ms | i 1025.6 MiB / r 475.4 MiB | i 126.91 mJ/MiB / r 151.84 mJ/MiB |
| RNS 1.4.2 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.25/s | 16.83 MB/s | 4030.00 / 4294.00 ms | i 409.1 MiB / r 9.5 MiB | i 164.88 mJ/MiB / r 51.51 mJ/MiB |

**Cell context**

1. **RNS 1.4.2 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.4.2's timing-sensitive handoff to a 65th 64-byte tail segment.

#### 64-segment resource stream · 1 Gbps policy (v3)

Stream 64 maximum-efficient resource segments with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 389/389 · 3/3 samples | 4/s | 288.72 MB/s | 232.00 / 245.00 ms | i 77.2 MiB / r 71.5 MiB | i 41.55 mJ/MiB / r 35.58 mJ/MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 105/105 · 3/3 samples | 1/s | 83.01 MB/s | 772.00 / 889.00 ms | i 139.7 MiB / r 779.3 MiB | i 54.51 mJ/MiB / r 195.26 mJ/MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 50/50 · 3/3 samples | 0.57/s | 38.24 MB/s | 1105.00 / 8768.00 ms | i 632.4 MiB / r 659.4 MiB | i 111.55 mJ/MiB / r 167.43 mJ/MiB |
| RNS 1.4.2 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 20/20 · 3/3 samples | 0.20/s | 13.33 MB/s | 4787.00 / 6924.00 ms | i 463.7 MiB / r 134.8 MiB | i 117.31 mJ/MiB / r 37.02 mJ/MiB |

**Cell context**

1. **RNS 1.4.2 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.4.2's timing-sensitive handoff to a 65th 64-byte tail segment.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15989/15989 · 3/3 samples | 178/s | 186.30 MB/s | 5.00 / 6.00 ms | i 42.4 MiB / r 39.6 MiB | i 56.98 mJ/MiB / r 45.55 mJ/MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 9214/9214 · 3/3 samples | 102/s | 107.12 MB/s | 10.00 / 11.00 ms | i 474.6 MiB / r 10.0 MiB | i 126.71 mJ/MiB / r 55.20 mJ/MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 5996/5996 · 3/3 samples | 67/s | 70.06 MB/s | 14.00 / 18.00 ms | i 12.0 MiB / r 408.5 MiB | i 52.50 mJ/MiB / r 161.47 mJ/MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 4780/4780 · 3/3 samples | 53/s | 55.64 MB/s | 18.00 / 22.00 ms | i 345.6 MiB / r 363.8 MiB | i 125.55 mJ/MiB / r 177.94 mJ/MiB |

#### Maximum resource segment · 1 Gbps policy (v1)

Repeat maximum-efficient resource transfers with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 16145/16145 · 3/3 samples | 178/s | 186.94 MB/s | 5.00 / 6.00 ms | i 74.1 MiB / r 71.8 MiB | i 57.20 mJ/MiB / r 45.28 mJ/MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 10200/10200 · 3/3 samples | 112/s | 117.68 MB/s | 8.00 / 10.00 ms | i 455.4 MiB / r 103.5 MiB | i 115.53 mJ/MiB / r 51.78 mJ/MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 6006/6006 · 3/3 samples | 66/s | 69.15 MB/s | 14.00 / 18.00 ms | i 74.3 MiB / r 660.0 MiB | i 57.96 mJ/MiB / r 191.22 mJ/MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 4829/4829 · 3/3 samples | 53/s | 56.04 MB/s | 18.00 / 22.00 ms | i 320.8 MiB / r 614.3 MiB | i 105.70 mJ/MiB / r 196.26 mJ/MiB |

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

### Transport

#### Raw transport throughput (v2)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 61925195/61925195 · 3/3 samples | 167.05 MB/s | 696.1k/s | 242.68 MB/s / 231.54 MB/s | 37.04 s | 44.4 MiB | 6.06× / 5.99× / 5.99× |
| RNS 1.4.2 (compiled) relay | 10 Mbps / 8 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 3039045/3039045 · 3/3 samples | 8.09 MB/s | 33.7k/s | 11.75 MB/s / 11.21 MB/s | 37.22 s | 323.4 MiB | 120.30× / 124.99× / 120.30× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal TCP policy: 500 Mbps for Prns and 10 Mbps for compiled RNS 1.4.2.

#### Transported resource throughput (v2)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default TCP policy.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | 128 / 128 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 534585/534585 · 3/3 samples | 777.74 MB/s | 5.9k/s | 784.01 MB/s / 784.01 MB/s | 31.16 s | 140.9 MiB | 8.61× / 7.36× / 7.36× |
| RNS 1.4.2 (compiled) relay | 10 Mbps / 8 KiB | 8 / 8 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 1779243/1779243 · 3/3 samples | 160.31 MB/s | 19.6k/s | 161.87 MB/s / 161.87 MB/s | 36.90 s | 257.5 MiB | 17.64× / 39.06× / 17.64× |

> Default-policy deployment view: Prns and RNS retain their normal TCP bitrate and MTU policy.

#### Transported resource throughput · 1 Gbps policy (v2)

Relay the identical transported-resource workload with both relay TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 119554/119554 · 3/3 samples | 691.07 MB/s | 1.3k/s | 696.54 MB/s / 696.54 MB/s | 30.84 s | 525.1 MiB | 8.22× / 6.28× / 6.28× |
| RNS 1.4.2 (compiled) relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 24735/24735 · 3/3 samples | 146.45 MB/s | 279/s | 147.61 MB/s / 147.61 MB/s | 34.26 s | 199.5 MiB | 38.56× / 30.38× / 30.38× |

> Controlled computational comparison: identical transported link and driver, with only TCP bitrate policy changed.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.2 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, actual HDLC-framed TCP wire rates, relay-only CPU/RSS, and full-path driver source/sink/limiting headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
