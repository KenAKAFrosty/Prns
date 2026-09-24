# Benchmark results — `x86_64-pc-windows-msvc`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `949135c64305df1d107d1b06a46ed1b18e2ee329`; source tree clean.

## Machine and method

AMD Ryzen 5 5600X 6-Core Processor; 6 physical / 12 logical; 31.9 GiB; Windows 11 Home.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Linux uses Backbone for both implementations in default-policy profiles. Policy-matched profiles use TCP because stock RNS Backbone fixes its policy at 100 Mbps / 32 KiB; the fixed-500-byte-MTU request profile also uses TCP because Backbone has no fixed-MTU setting. macOS and Windows use TCP, the stock RNS fallback on hosts without Backbone support. Default-policy rows preserve each implementation's normal bitrate and MTU policy. Policy-matched resource rows configure both implementations for RNS TCP's 10 Mbps / 16 KiB tier; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-x86_64-pc-windows-msvc-dark.svg">
  <img alt="Bar chart of Prns median throughput as a multiple of RNS 1.5.4 for each published scenario" src="assets/at-a-glance-x86_64-pc-windows-msvc-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-memory-x86_64-pc-windows-msvc-dark.svg">
  <img alt="Bar chart of RNS 1.5.4 peak memory as a multiple of Prns for each role and scenario" src="assets/at-a-glance-memory-x86_64-pc-windows-msvc-light.svg">
</picture>

<details>
<summary>Chart data as a table</summary>

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 32.2k/s | 676/s | 47.62× |
| Link-message throughput | 76.3k/s | 4.2k/s | 18.37× |
| Request/response | 18.7k/s | 310/s | 60.46× |
| Maximum resource segment | 200.48 MB/s | 41.39 MB/s | 4.84× |
| Maximum resource segment · matched RNS policy | 206.35 MB/s | 41.37 MB/s | 4.99× |
| 64-segment resource stream | 438.18 MB/s | 49.46 MB/s | 8.86× |
| 64-segment resource stream · matched RNS policy | 479.11 MB/s | 49.55 MB/s | 9.67× |
| Raw transport throughput | 171.87 MB/s | 3.98 MB/s | 43.16× |
| Transported resource throughput | 143.16 MB/s | 141.03 MB/s | 1.02× |
| Transported resource throughput · matched RNS policy | 1344.99 MB/s | 140.43 MB/s | 9.58× |

| Scenario · role | Prns peak RSS | Reference peak RSS | Reference / Prns |
|---|---:|---:|---:|
| Single-packet throughput · initiator | 12.6 MiB | 47.9 MiB | 3.80× |
| Single-packet throughput · responder | 49.6 MiB | 48.0 MiB | 0.97× |
| Link-message throughput · initiator | 13.4 MiB | 77.8 MiB | 5.80× |
| Link-message throughput · responder | 49.7 MiB | 70.4 MiB | 1.41× |
| Request/response · initiator | 40.2 MiB | 71.1 MiB | 1.77× |
| Request/response · responder | 31.3 MiB | 83.3 MiB | 2.66× |
| Maximum resource segment · initiator | 15.9 MiB | 146.6 MiB | 9.23× |
| Maximum resource segment · responder | 15.5 MiB | 71.0 MiB | 4.58× |
| Maximum resource segment · matched RNS policy · initiator | 14.9 MiB | 144.3 MiB | 9.72× |
| Maximum resource segment · matched RNS policy · responder | 14.0 MiB | 71.1 MiB | 5.09× |
| 64-segment resource stream · initiator | 19.5 MiB | 132.8 MiB | 6.82× |
| 64-segment resource stream · responder | 15.7 MiB | 194.7 MiB | 12.42× |
| 64-segment resource stream · matched RNS policy · initiator | 18.4 MiB | 133.0 MiB | 7.21× |
| 64-segment resource stream · matched RNS policy · responder | 14.2 MiB | 186.9 MiB | 13.17× |
| Raw transport throughput · relay | 49.8 MiB | 152.8 MiB | 3.07× |
| Transported resource throughput · relay | 20.5 MiB | 72.8 MiB | 3.54× |
| Transported resource throughput · matched RNS policy · relay | 11.4 MiB | 73.3 MiB | 6.42× |

A dash means no current three-sample release evidence is published for that scenario.

</details>

## Detailed results

### Links

#### Link-message throughput (v8)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 6870731/6870731 · 3/3 samples | 76.3k/s | 18.32 MB/s | <1.00 / 1.00 ms | i 13.4 MiB / r 49.7 MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 805817/805817 · 3/3 samples | 9.0k/s | 2.15 MB/s | 2.00 / 2.00 ms | i 116.8 MiB / r 28.7 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 431829/431829 · 3/3 samples | 4.8k/s | 1.15 MB/s | 3.00 / 4.00 ms | i 12.0 MiB / r 74.0 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 372939/372939 · 3/3 samples | 4.2k/s | 996.8 kB/s | 4.00 / 4.00 ms | i 77.8 MiB / r 70.4 MiB |

### Packets

#### Single-packet throughput (v6)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2877145/2877145 · 3/3 samples | 32.2k/s | 7.08 MB/s | <1.00 / 1.00 ms | i 12.6 MiB / r 49.6 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 570647/570647 · 3/3 samples | 6.3k/s | 1.39 MB/s | 2.00 / 3.00 ms | i 11.3 MiB / r 91.2 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 60816/60816 · 3/3 samples | 676/s | 148.7 kB/s | 23.00 / 25.00 ms | i 47.9 MiB / r 48.0 MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 60379/60379 · 3/3 samples | 670/s | 147.4 kB/s | 24.00 / 26.00 ms | i 48.2 MiB / r 13.2 MiB |

### Requests

#### Request/response (v12)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1682951/1682951 · 3/3 samples | 18.7k/s | 0.21 / 0.27 ms | i 40.2 MiB / r 31.3 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 60572/60572 · 3/3 samples | 667/s | 2.22 / 252.21 ms | i 10.6 MiB / r 125.4 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 28436/28436 · 3/3 samples | 310/s | 8.90 / 143.23 ms | i 71.1 MiB / r 83.3 MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 23001/23001 · 3/3 samples | 245/s | 6.17 / 119.67 ms | i 66.9 MiB / r 9.6 MiB |

### Resources

#### 64-segment resource stream (v10)

Stream 64 maximum-efficient resource segments with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 589/589 · 3/3 samples | 7/s | 438.18 MB/s | 149.00 / 207.00 ms | i 19.5 MiB / r 15.7 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 56/56 · 3/3 samples | 0.74/s | 49.46 MB/s | 1270.00 / 1764.00 ms | i 132.8 MiB / r 194.7 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 43/43 · 3/3 samples | 0.43/s | 29.06 MB/s | 2283.00 / 2684.00 ms | i 19.0 MiB / r 236.9 MiB |
| RNS 1.5.4 → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 22/22 · 3/3 samples | 0.25/s | 16.81 MB/s | 4028.00 / 4262.00 ms | i 122.2 MiB / r 13.0 MiB |

**Cell context**

1. **RNS 1.5.4 → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.5.4's timing-sensitive handoff to a 65th 64-byte tail segment.

#### 64-segment resource stream · matched RNS policy (v1)

Stream 64 maximum-efficient resource segments with both endpoint interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 637/637 · 3/3 samples | 7/s | 479.11 MB/s | 138.00 / 160.00 ms | i 18.4 MiB / r 14.2 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 68/68 · 3/3 samples | 0.74/s | 49.55 MB/s | 1268.00 / 1926.00 ms | i 133.0 MiB / r 186.9 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 44/44 · 3/3 samples | 0.45/s | 29.96 MB/s | 2062.00 / 2735.00 ms | i 18.7 MiB / r 225.0 MiB |
| RNS 1.5.4 → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 23/23 · 3/3 samples | 0.25/s | 16.74 MB/s | 4027.00 / 4233.00 ms | i 123.2 MiB / r 12.7 MiB |

**Cell context**

1. **RNS 1.5.4 → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled policy comparison: identical workload and protocol with both implementations configured for the stock RNS bitrate and MTU tier.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.5.4's timing-sensitive handoff to a 65th 64-byte tail segment.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 17171/17171 · 3/3 samples | 191/s | 200.48 MB/s | 5.00 / 6.00 ms | i 15.9 MiB / r 15.5 MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8602/8602 · 3/3 samples | 94/s | 99.05 MB/s | 10.00 / 12.00 ms | i 285.3 MiB / r 13.5 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 4347/4347 · 3/3 samples | 48/s | 50.60 MB/s | 15.00 / 18.00 ms | i 14.7 MiB / r 76.1 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 3208/3208 · 3/3 samples | 39/s | 41.39 MB/s | 20.00 / 23.00 ms | i 146.6 MiB / r 71.0 MiB |

#### Maximum resource segment · matched RNS policy (v1)

Repeat maximum-efficient resource transfers with both endpoint interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 17732/17732 · 3/3 samples | 197/s | 206.35 MB/s | 5.00 / 5.00 ms | i 14.9 MiB / r 14.0 MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8570/8570 · 3/3 samples | 95/s | 99.27 MB/s | 10.00 / 12.00 ms | i 287.1 MiB / r 13.3 MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 4170/4170 · 3/3 samples | 46/s | 48.52 MB/s | 15.00 / 17.00 ms | i 14.4 MiB / r 74.6 MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 3523/3523 · 3/3 samples | 39/s | 41.37 MB/s | 20.00 / 24.00 ms | i 144.3 MiB / r 71.1 MiB |

> Controlled policy comparison: identical workload and protocol with both implementations configured for the stock RNS bitrate and MTU tier.

### Transport

#### Raw transport throughput (v2)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 64512011/64512011 · 3/3 samples | 171.87 MB/s | 716.2k/s | 249.67 MB/s / 238.21 MB/s | 33.62 s | 49.8 MiB | 7.67× / 4.05× / 4.05× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 1500643/1500643 · 3/3 samples | 3.98 MB/s | 16.6k/s | 5.78 MB/s / 5.52 MB/s | 35.61 s | 152.8 MiB | 326.24× / 185.35× / 185.35× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal host-interface policy; the exact bitrate and MTU are reported with every row.

#### Transported resource throughput (v2)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default host-interface policy.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | 128 / 128 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 96400/96400 · 3/3 samples | 143.16 MB/s | 1.1k/s | 144.31 MB/s / 144.31 MB/s | 3.78 s | 20.5 MiB | 48.63× / 5.19× / 5.19× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 777543/777543 · 3/3 samples | 141.03 MB/s | 8.6k/s | 142.32 MB/s / 142.32 MB/s | 40.89 s | 72.8 MiB | 25.85× / 33.95× / 25.85× |

> Default-policy deployment view: Prns and RNS retain their normal host-interface bitrate and MTU policy.

#### Transported resource throughput · matched RNS policy (v1)

Relay the identical transported-resource workload with both relay interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 7438301/7438301 · 3/3 samples | 1344.99 MB/s | 82.2k/s | 1357.30 MB/s / 1357.30 MB/s | 27.55 s | 11.4 MiB | 2.68× / 3.24× / 2.68× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 772034/772034 · 3/3 samples | 140.43 MB/s | 8.6k/s | 141.71 MB/s / 141.71 MB/s | 40.95 s | 73.3 MiB | 25.81× / 32.63× / 25.81× |

> Controlled policy comparison: identical transported link and driver with both implementations configured for the stock RNS bitrate and MTU tier.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.5.4** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, actual HDLC-framed TCP wire rates, relay-only CPU/RSS, and full-path driver source/sink/limiting headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
