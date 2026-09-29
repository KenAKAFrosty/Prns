# Benchmark results — `x86_64-unknown-linux-gnu`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `43cb3416d45d2b8188284a3ba81609ab65e9bfaa`; source tree clean.

## Machine and method

12th Gen Intel(R) Core(TM) i7-1260P; 12 physical / 16 logical; 31.0 GiB; Linux (Ubuntu 24.04).

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Linux uses Backbone for both implementations in default-policy profiles. Policy-matched profiles use TCP because stock RNS Backbone fixes its policy at 100 Mbps / 32 KiB; the fixed-500-byte-MTU request profile also uses TCP because Backbone has no fixed-MTU setting. macOS and Windows use TCP, the stock RNS fallback on hosts without Backbone support. Default-policy rows preserve each implementation's normal bitrate and MTU policy. Policy-matched resource rows configure both implementations for RNS TCP's 10 Mbps / 16 KiB tier; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-x86_64-unknown-linux-gnu-dark.svg">
  <img alt="Bar chart of Prns median throughput as a multiple of RNS 1.5.4 for each published scenario" src="assets/at-a-glance-x86_64-unknown-linux-gnu-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-memory-x86_64-unknown-linux-gnu-dark.svg">
  <img alt="Bar chart of RNS 1.5.4 peak memory as a multiple of Prns for each role and scenario" src="assets/at-a-glance-memory-x86_64-unknown-linux-gnu-light.svg">
</picture>

<details>
<summary>Chart data as a table</summary>

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 19.2k/s | 711/s | 26.97× |
| Link-message throughput | 65.5k/s | 7.1k/s | 9.28× |
| Request/response | 27.3k/s | 630/s | 43.33× |
| Maximum resource segment | 175.67 MB/s | 58.70 MB/s | 2.99× |
| Maximum resource segment · matched RNS policy | 175.29 MB/s | 62.55 MB/s | 2.80× |
| 64-segment resource stream | 363.22 MB/s | 67.38 MB/s | 5.39× |
| 64-segment resource stream · matched RNS policy | 427.23 MB/s | 56.01 MB/s | 7.63× |
| Raw transport throughput | 160.78 MB/s | 10.14 MB/s | 15.85× |
| Transported resource throughput | 1476.09 MB/s | 192.51 MB/s | 7.67× |
| Transported resource throughput · matched RNS policy | 1735.42 MB/s | 172.27 MB/s | 10.07× |

| Scenario · role | Prns peak RSS | Reference peak RSS | Reference / Prns |
|---|---:|---:|---:|
| Single-packet throughput · initiator | 7.3 MiB | 47.2 MiB | 6.43× |
| Single-packet throughput · responder | 31.3 MiB | 47.7 MiB | 1.53× |
| Link-message throughput · initiator | 7.5 MiB | 102.7 MiB | 13.72× |
| Link-message throughput · responder | 45.6 MiB | 94.1 MiB | 2.07× |
| Request/response · initiator | 50.5 MiB | 94.2 MiB | 1.87× |
| Request/response · responder | 37.7 MiB | 134.8 MiB | 3.57× |
| Maximum resource segment · initiator | 14.7 MiB | 176.3 MiB | 12.01× |
| Maximum resource segment · responder | 13.5 MiB | 109.7 MiB | 8.13× |
| Maximum resource segment · matched RNS policy · initiator | 13.2 MiB | 200.4 MiB | 15.18× |
| Maximum resource segment · matched RNS policy · responder | 11.1 MiB | 219.1 MiB | 19.82× |
| 64-segment resource stream · initiator | 18.2 MiB | 393.2 MiB | 21.61× |
| 64-segment resource stream · responder | 13.3 MiB | 223.8 MiB | 16.85× |
| 64-segment resource stream · matched RNS policy · initiator | 16.4 MiB | 593.6 MiB | 36.20× |
| 64-segment resource stream · matched RNS policy · responder | 11.5 MiB | 341.4 MiB | 29.62× |
| Raw transport throughput · relay | 45.9 MiB | 180.1 MiB | 3.92× |
| Transported resource throughput · relay | 42.9 MiB | 67.1 MiB | 1.57× |
| Transported resource throughput · matched RNS policy · relay | 8.6 MiB | 92.2 MiB | 10.71× |

A dash means no current three-sample release evidence is published for that scenario.

</details>

## Detailed results

### Links

#### Link-message throughput (v8)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 5761337/5761337 · 3/3 samples | 65.5k/s | 15.72 MB/s | <1.00 / 1.00 ms | i 7.5 MiB / r 45.6 MiB | i 0.23 mJ / r 0.19 mJ |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 818474/818474 · 3/3 samples | 9.1k/s | 2.19 MB/s | 2.00 / 2.00 ms | i 117.2 MiB / r 17.2 MiB | i 1.25 mJ / r 1.43 mJ |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 649488/649488 · 3/3 samples | 7.2k/s | 1.73 MB/s | 2.00 / 4.00 ms | i 7.5 MiB / r 93.9 MiB | i 2.18 mJ / r 1.39 mJ |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 640333/640333 · 3/3 samples | 7.1k/s | 1.69 MB/s | 2.00 / 3.00 ms | i 102.7 MiB / r 94.1 MiB | i 1.92 mJ / r 1.82 mJ |

### Packets

#### Single-packet throughput (v6)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 1789847/1789847 · 3/3 samples | 19.2k/s | 4.22 MB/s | 1.00 / 1.00 ms | i 7.3 MiB / r 31.3 MiB | i 0.72 mJ / r 0.61 mJ |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 860830/860830 · 3/3 samples | 9.5k/s | 2.10 MB/s | 2.00 / 3.00 ms | i 7.2 MiB / r 106.7 MiB | i 1.78 mJ / r 1.01 mJ |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 63513/63513 · 3/3 samples | 711/s | 156.3 kB/s | 22.00 / 25.00 ms | i 47.2 MiB / r 47.7 MiB | i 21.88 mJ / r 2.88 mJ |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 62624/62624 · 3/3 samples | 694/s | 152.5 kB/s | 23.00 / 25.00 ms | i 47.3 MiB / r 7.5 MiB | i 18.17 mJ / r 9.14 mJ |

### Requests

#### Request/response (v12)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2455955/2455955 · 3/3 samples | 27.3k/s | 0.14 / 0.22 ms | i 50.5 MiB / r 37.7 MiB | i 0.29 mJ / r 0.31 mJ |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 58509/58509 · 3/3 samples | 640/s | 0.80 / 3.52 ms | i 103.3 MiB / r 7.3 MiB | i 15.34 mJ / r 1.85 mJ |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 56902/56902 · 3/3 samples | 630/s | 4.20 / 12.15 ms | i 94.2 MiB / r 134.8 MiB | i 11.28 mJ / r 19.93 mJ |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 54100/54100 · 3/3 samples | 587/s | 2.27 / 252.69 ms | i 7.6 MiB / r 130.4 MiB | i 1.59 mJ / r 23.41 mJ |

### Resources

#### 64-segment resource stream (v10)

Stream 64 maximum-efficient resource segments with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 486/486 · 3/3 samples | 5/s | 363.22 MB/s | 184.00 / 205.00 ms | i 18.2 MiB / r 13.3 MiB | i 35.29 mJ/MiB / r 35.05 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 119/119 · 3/3 samples | 1/s | 88.80 MB/s | 741.00 / 838.00 ms | i 16.6 MiB / r 408.2 MiB | i 50.02 mJ/MiB / r 156.11 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 92/92 · 3/3 samples | 1/s | 67.38 MB/s | 917.00 / 1420.00 ms | i 393.2 MiB / r 223.8 MiB | i 124.17 mJ/MiB / r 148.28 mJ/MiB |
| RNS 1.5.4 → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 27/27 · 3/3 samples | 0.28/s | 18.77 MB/s | 3598.00 / 3653.00 ms | i 217.6 MiB / r 10.7 MiB | i 162.15 mJ/MiB / r 87.02 mJ/MiB |

**Cell context**

1. **RNS 1.5.4 → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.5.4's timing-sensitive handoff to a 65th 64-byte tail segment.

#### 64-segment resource stream · matched RNS policy (v1)

Stream 64 maximum-efficient resource segments with both endpoint interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 575/575 · 3/3 samples | 6/s | 427.23 MB/s | 155.00 / 181.00 ms | i 16.4 MiB / r 11.5 MiB | i 28.42 mJ/MiB / r 34.25 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 118/118 · 3/3 samples | 1/s | 86.55 MB/s | 737.00 / 1046.00 ms | i 16.7 MiB / r 453.1 MiB | i 49.57 mJ/MiB / r 154.02 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 77/77 · 3/3 samples | 0.83/s | 56.01 MB/s | 1110.00 / 1834.00 ms | i 593.6 MiB / r 341.4 MiB | i 121.82 mJ/MiB / r 157.17 mJ/MiB |
| RNS 1.5.4 → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 27/27 · 3/3 samples | 0.28/s | 18.59 MB/s | 3608.00 / 3754.00 ms | i 217.3 MiB / r 10.6 MiB | i 166.92 mJ/MiB / r 101.90 mJ/MiB |

**Cell context**

1. **RNS 1.5.4 → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled policy comparison: identical workload and protocol with both implementations configured for the stock RNS bitrate and MTU tier.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.5.4's timing-sensitive handoff to a 65th 64-byte tail segment.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15070/15070 · 3/3 samples | 168/s | 175.67 MB/s | 5.00 / 6.00 ms | i 14.7 MiB / r 13.5 MiB | i 44.26 mJ/MiB / r 43.46 mJ/MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 10132/10132 · 3/3 samples | 113/s | 118.07 MB/s | 9.00 / 11.00 ms | i 312.9 MiB / r 10.8 MiB | i 109.63 mJ/MiB / r 79.77 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 6124/6124 · 3/3 samples | 67/s | 70.77 MB/s | 14.00 / 16.00 ms | i 13.4 MiB / r 223.5 MiB | i 54.86 mJ/MiB / r 165.75 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 5021/5021 · 3/3 samples | 56/s | 58.70 MB/s | 17.00 / 20.00 ms | i 176.3 MiB / r 109.7 MiB | i 122.69 mJ/MiB / r 171.56 mJ/MiB |

#### Maximum resource segment · matched RNS policy (v1)

Repeat maximum-efficient resource transfers with both endpoint interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15066/15066 · 3/3 samples | 167/s | 175.29 MB/s | 5.00 / 7.00 ms | i 13.2 MiB / r 11.1 MiB | i 41.89 mJ/MiB / r 58.31 mJ/MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 11212/11212 · 3/3 samples | 125/s | 130.91 MB/s | 8.00 / 10.00 ms | i 363.4 MiB / r 11.4 MiB | i 106.19 mJ/MiB / r 72.53 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 6255/6255 · 3/3 samples | 69/s | 72.68 MB/s | 13.00 / 16.00 ms | i 13.1 MiB / r 253.5 MiB | i 57.04 mJ/MiB / r 172.08 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 5373/5373 · 3/3 samples | 60/s | 62.55 MB/s | 16.00 / 20.00 ms | i 200.4 MiB / r 219.1 MiB | i 132.63 mJ/MiB / r 185.83 mJ/MiB |

> Controlled policy comparison: identical workload and protocol with both implementations configured for the stock RNS bitrate and MTU tier.

### Transport

#### Raw transport throughput (v2)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 1 Gbps / 512 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 59993795/59993795 · 3/3 samples | 160.78 MB/s | 670.0k/s | 233.57 MB/s / 222.85 MB/s | 37.07 s | 45.9 MiB | 6.28× / 8.24× / 6.28× |
| RNS 1.5.4 relay | 100 Mbps / 32 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 3787213/3787213 · 3/3 samples | 10.14 MB/s | 42.3k/s | 14.73 MB/s / 14.06 MB/s | 33.53 s | 180.1 MiB | 98.26× / 138.41× / 98.26× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal host-interface policy; the exact bitrate and MTU are reported with every row.

#### Transported resource throughput (v2)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default host-interface policy.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 253184/253184 · 3/3 samples | 1476.09 MB/s | 2.8k/s | 1487.77 MB/s / 1487.77 MB/s | 32.16 s | 42.9 MiB | 4.22× / 1.88× / 1.88× |
| RNS 1.5.4 relay | 100 Mbps / 32 KiB | 32 / 32 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 528756/528756 · 3/3 samples | 192.51 MB/s | 5.9k/s | 194.11 MB/s / 194.11 MB/s | 31.91 s | 67.1 MiB | 35.02× / 27.68× / 27.68× |

> Default-policy deployment view: Prns and RNS retain their normal host-interface bitrate and MTU policy.

#### Transported resource throughput · matched RNS policy (v1)

Relay the identical transported-resource workload with both relay interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 9547003/9547003 · 3/3 samples | 1735.42 MB/s | 106.1k/s | 1751.29 MB/s / 1751.29 MB/s | 37.53 s | 8.6 MiB | 2.85× / 4.05× / 2.85× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 950288/950288 · 3/3 samples | 172.27 MB/s | 10.5k/s | 173.84 MB/s / 173.84 MB/s | 38.12 s | 92.2 MiB | 26.92× / 26.08× / 26.08× |

> Controlled policy comparison: identical transported link and driver with both implementations configured for the stock RNS bitrate and MTU tier.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.5.4** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, actual HDLC-framed TCP wire rates, relay-only CPU/RSS, and full-path driver source/sink/limiting headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
