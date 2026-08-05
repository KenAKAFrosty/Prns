# Benchmark results — `x86_64-pc-windows-msvc`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `d18cfb145f9fef114b366f4132da75513feb8022`; source tree clean.

## Machine and method

AMD Ryzen 5 5600X 6-Core Processor; 6 physical / 12 logical; 31.9 GiB; Windows 11 Home.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Default-policy rows preserve each implementation's normal TCP bitrate and MTU policy. The controlled 1 Gbps resource rows change only that interface policy, both for real endpoint transfers and for transported-resource switching; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-x86_64-pc-windows-msvc-dark.svg">
  <img alt="Bar chart of Prns median throughput as a multiple of RNS 1.4.2 (compiled) for each published scenario" src="assets/at-a-glance-x86_64-pc-windows-msvc-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-memory-x86_64-pc-windows-msvc-dark.svg">
  <img alt="Bar chart of RNS 1.4.2 (compiled) peak memory as a multiple of Prns for each role and scenario" src="assets/at-a-glance-memory-x86_64-pc-windows-msvc-light.svg">
</picture>

<details>
<summary>Chart data as a table</summary>

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 30.8k/s | 3.9k/s | 8.00× |
| Link-message throughput | 44.3k/s | 4.2k/s | 10.65× |
| Request/response | 4.5k/s | 284/s | 15.73× |
| Maximum resource segment | 94.39 MB/s | 39.72 MB/s | 2.38× |
| Maximum resource segment · 1 Gbps policy | 177.03 MB/s | 23.16 MB/s | 7.64× |
| 64-segment resource stream | 123.72 MB/s | 51.72 MB/s | 2.39× |
| 64-segment resource stream · 1 Gbps policy | 259.60 MB/s | 30.76 MB/s | 8.44× |
| Raw transport throughput | 147.22 MB/s | 3.79 MB/s | 38.84× |
| Transported resource throughput | 157.51 MB/s | 123.90 MB/s | 1.27× |
| Transported resource throughput · 1 Gbps policy | 439.60 MB/s | 94.41 MB/s | 4.66× |

| Scenario · role | Prns peak RSS | Reference peak RSS | Reference / Prns |
|---|---:|---:|---:|
| Single-packet throughput · initiator | 9.0 MiB | 211.0 MiB | 23.51× |
| Single-packet throughput · responder | 46.7 MiB | 217.4 MiB | 4.66× |
| Link-message throughput · initiator | 10.2 MiB | 226.1 MiB | 22.09× |
| Link-message throughput · responder | 47.8 MiB | 218.5 MiB | 4.57× |
| Request/response · initiator | 19.9 MiB | 214.9 MiB | 10.80× |
| Request/response · responder | 20.2 MiB | 226.7 MiB | 11.25× |
| Maximum resource segment · initiator | 45.3 MiB | 301.2 MiB | 6.66× |
| Maximum resource segment · responder | 49.8 MiB | 224.0 MiB | 4.50× |
| Maximum resource segment · 1 Gbps policy · initiator | 78.6 MiB | 251.2 MiB | 3.20× |
| Maximum resource segment · 1 Gbps policy · responder | 83.5 MiB | 225.6 MiB | 2.70× |
| 64-segment resource stream · initiator | 47.1 MiB | 293.2 MiB | 6.22× |
| 64-segment resource stream · responder | 49.0 MiB | 364.0 MiB | 7.43× |
| 64-segment resource stream · 1 Gbps policy · initiator | 81.2 MiB | 269.1 MiB | 3.31× |
| 64-segment resource stream · 1 Gbps policy · responder | 81.5 MiB | 400.2 MiB | 4.91× |
| Raw transport throughput · relay | 48.6 MiB | 296.3 MiB | 6.09× |
| Transported resource throughput · relay | 148.4 MiB | 243.0 MiB | 1.64× |
| Transported resource throughput · 1 Gbps policy · relay | 539.2 MiB | 198.8 MiB | 0.37× |

A dash means no current three-sample release evidence is published for that scenario.

</details>

## Detailed results

### Links

#### Link-message throughput (v8)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 4006922/4006922 · 3/3 samples | 44.3k/s | 10.64 MB/s | <1.00 / 1.00 ms | i 10.2 MiB / r 47.8 MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 396912/396912 · 3/3 samples | 4.4k/s | 1.06 MB/s | <1.00 / <1.00 ms | i 228.2 MiB / r 18.1 MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 380473/380473 · 3/3 samples | 4.2k/s | 1.02 MB/s | 4.00 / 4.00 ms | i 10.1 MiB / r 218.8 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 374887/374887 · 3/3 samples | 4.2k/s | 998.6 kB/s | 4.00 / 4.00 ms | i 226.1 MiB / r 218.5 MiB |

### Packets

#### Single-packet throughput (v6)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2777585/2777585 · 3/3 samples | 30.8k/s | 6.78 MB/s | <1.00 / 1.00 ms | i 9.0 MiB / r 46.7 MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 599852/599852 · 3/3 samples | 6.7k/s | 1.47 MB/s | 2.00 / 3.00 ms | i 8.8 MiB / r 239.1 MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 345931/345931 · 3/3 samples | 3.9k/s | 849.1 kB/s | <1.00 / 1.00 ms | i 211.0 MiB / r 14.6 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 346200/346200 · 3/3 samples | 3.9k/s | 846.9 kB/s | <1.00 / 1.00 ms | i 211.0 MiB / r 217.4 MiB |

### Requests

#### Request/response (v12)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 400318/400318 · 3/3 samples | 4.5k/s | 0.90 / 1.35 ms | i 19.9 MiB / r 20.2 MiB |
| Prns → RNS 1.4.2 (compiled)<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 49220/49220 · 3/3 samples | 549/s | 1.89 / 254.31 ms | i 10.3 MiB / r 264.7 MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 30313/30313 · 3/3 samples | 332/s | 9.36 / 72.31 ms | i 218.3 MiB / r 11.3 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 26087/26087 · 3/3 samples | 284/s | 9.90 / 159.96 ms | i 214.9 MiB / r 226.7 MiB |

**Cell context**

1. **Prns → RNS 1.4.2 (compiled)** — RNS sends a resource advertisement before registering that resource internally. Prns can return the first pull so quickly that RNS drops it. Prns then waits for its 250 ms retry deadline. A p99 pinned just above 250 ms is the fingerprint of this race.

### Resources

#### 64-segment resource stream (v10)

Stream 64 maximum-efficient resource segments with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 165/165 · 3/3 samples | 2/s | 123.72 MB/s | 549.00 / 658.00 ms | i 47.1 MiB / r 49.0 MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 84/84 · 3/3 samples | 1/s | 75.83 MB/s | 808.00 / 1193.00 ms | i 16.9 MiB / r 370.8 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 71/71 · 3/3 samples | 0.77/s | 51.72 MB/s | 1197.00 / 1671.00 ms | i 293.2 MiB / r 364.0 MiB |
| RNS 1.4.2 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 21/21 · 3/3 samples | 0.23/s | 15.55 MB/s | 3932.00 / 6294.00 ms | i 273.7 MiB / r 12.9 MiB |

**Cell context**

1. **RNS 1.4.2 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.4.2's timing-sensitive handoff to a 65th 64-byte tail segment.

#### 64-segment resource stream · 1 Gbps policy (v3)

Stream 64 maximum-efficient resource segments with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 350/350 · 3/3 samples | 4/s | 259.60 MB/s | 256.00 / 282.00 ms | i 81.2 MiB / r 81.5 MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 77/77 · 3/3 samples | 0.80/s | 53.91 MB/s | 1029.00 / 4875.00 ms | i 144.3 MiB / r 406.2 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 44/44 · 3/3 samples | 0.46/s | 30.76 MB/s | 1922.00 / 5324.00 ms | i 269.1 MiB / r 400.2 MiB |
| RNS 1.4.2 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.26/s | 17.64 MB/s | 3799.00 / 3914.00 ms | i 267.4 MiB / r 116.0 MiB |

**Cell context**

1. **RNS 1.4.2 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.4.2's timing-sensitive handoff to a 65th 64-byte tail segment.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8147/8147 · 3/3 samples | 90/s | 94.39 MB/s | 8.00 / 26.00 ms | i 45.3 MiB / r 49.8 MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 7621/7621 · 3/3 samples | 85/s | 89.35 MB/s | 12.00 / 13.00 ms | i 436.6 MiB / r 13.4 MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 4217/4217 · 3/3 samples | 47/s | 49.11 MB/s | 14.00 / 23.00 ms | i 14.6 MiB / r 234.0 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 3352/3352 · 3/3 samples | 38/s | 39.72 MB/s | 19.00 / 22.00 ms | i 301.2 MiB / r 224.0 MiB |

#### Maximum resource segment · 1 Gbps policy (v1)

Repeat maximum-efficient resource transfers with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15194/15194 · 3/3 samples | 169/s | 177.03 MB/s | 5.00 / 6.00 ms | i 78.6 MiB / r 83.5 MiB |
| RNS 1.4.2 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 7784/7784 · 3/3 samples | 88/s | 92.12 MB/s | 11.00 / 13.00 ms | i 400.5 MiB / r 149.6 MiB |
| Prns → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 3391/3391 · 3/3 samples | 38/s | 39.35 MB/s | 18.00 / 37.00 ms | i 78.2 MiB / r 242.0 MiB |
| RNS 1.4.2 (compiled) → RNS 1.4.2 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 1992/1992 · 3/3 samples | 22/s | 23.16 MB/s | 40.00 / 43.00 ms | i 251.2 MiB / r 225.6 MiB |

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

### Transport

#### Raw transport throughput (v2)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 55216111/55216111 · 3/3 samples | 147.22 MB/s | 613.5k/s | 213.88 MB/s / 204.06 MB/s | 35.80 s | 48.6 MiB | 10.15× / 4.82× / 4.82× |
| RNS 1.4.2 (compiled) relay | 10 Mbps / 8 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 1422221/1422221 · 3/3 samples | 3.79 MB/s | 15.8k/s | 5.51 MB/s / 5.25 MB/s | 46.55 s | 296.3 MiB | 391.97× / 199.92× / 199.92× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal TCP policy: 500 Mbps for Prns and 10 Mbps for compiled RNS 1.4.2.

#### Transported resource throughput (v2)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default TCP policy.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | 128 / 128 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 109031/109031 · 3/3 samples | 157.51 MB/s | 1.2k/s | 158.78 MB/s / 158.78 MB/s | 6.62 s | 148.4 MiB | 31.57× / 5.21× / 5.21× |
| RNS 1.4.2 (compiled) relay | 10 Mbps / 8 KiB | 8 / 8 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 1363949/1363949 · 3/3 samples | 123.90 MB/s | 15.2k/s | 125.10 MB/s / 125.10 MB/s | 44.72 s | 243.0 MiB | 9.26× / 39.30× / 9.26× |

> Default-policy deployment view: Prns and RNS retain their normal TCP bitrate and MTU policy.

#### Transported resource throughput · 1 Gbps policy (v2)

Relay the identical transported-resource workload with both relay TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 76077/76077 · 3/3 samples | 439.60 MB/s | 838/s | 443.08 MB/s / 443.08 MB/s | 19.34 s | 539.2 MiB | 9.44× / 4.85× / 4.85× |
| RNS 1.4.2 (compiled) relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 16278/16278 · 3/3 samples | 94.41 MB/s | 180/s | 95.15 MB/s / 95.15 MB/s | 35.11 s | 198.8 MiB | 44.11× / 21.28× / 21.28× |

> Controlled computational comparison: identical transported link and driver, with only TCP bitrate policy changed.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.2 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, actual HDLC-framed TCP wire rates, relay-only CPU/RSS, and full-path driver source/sink/limiting headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
