# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `d253e964fb7659d3b3777d84d035cd73367f91fe`; source tree clean.

## Machine and method

Apple M4; 10 physical / 10 logical; 16.0 GiB; macOS 26.4.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Linux uses Backbone for both implementations in default-policy profiles. Policy-matched profiles use TCP because stock RNS Backbone fixes its policy at 100 Mbps / 32 KiB; the fixed-500-byte-MTU request profile also uses TCP because Backbone has no fixed-MTU setting. macOS and Windows use TCP, the stock RNS fallback on hosts without Backbone support. Default-policy rows preserve each implementation's normal bitrate and MTU policy. Policy-matched resource rows configure both implementations for RNS TCP's 10 Mbps / 16 KiB tier; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-aarch64-apple-darwin-dark.svg">
  <img alt="Bar chart of Prns median throughput as a multiple of RNS 1.5.4 for each published scenario" src="assets/at-a-glance-aarch64-apple-darwin-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-memory-aarch64-apple-darwin-dark.svg">
  <img alt="Bar chart of RNS 1.5.4 peak memory as a multiple of Prns for each role and scenario" src="assets/at-a-glance-memory-aarch64-apple-darwin-light.svg">
</picture>

<details>
<summary>Chart data as a table</summary>

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 43.8k/s | 398/s | 110.22× |
| Link-message throughput | 78.3k/s | 6.0k/s | 13.15× |
| Request/response | 30.8k/s | 1.1k/s | 28.15× |
| Maximum resource segment | 330.36 MB/s | 99.98 MB/s | 3.30× |
| Maximum resource segment · matched RNS policy | 336.05 MB/s | 100.37 MB/s | 3.35× |
| 64-segment resource stream | 680.01 MB/s | 123.44 MB/s | 5.51× |
| 64-segment resource stream · matched RNS policy | 699.66 MB/s | 125.41 MB/s | 5.58× |
| Raw transport throughput | 242.97 MB/s | 9.59 MB/s | 25.33× |
| Transported resource throughput | 3318.78 MB/s | 156.76 MB/s | 21.17× |
| Transported resource throughput · matched RNS policy | 2781.48 MB/s | 156.86 MB/s | 17.73× |

| Scenario · role | Prns peak RSS | Reference peak RSS | Reference / Prns |
|---|---:|---:|---:|
| Single-packet throughput · initiator | 6.6 MiB | 47.3 MiB | 7.21× |
| Single-packet throughput · responder | 49.6 MiB | 49.3 MiB | 0.99× |
| Link-message throughput · initiator | 6.8 MiB | 112.8 MiB | 16.60× |
| Link-message throughput · responder | 50.5 MiB | 103.1 MiB | 2.04× |
| Request/response · initiator | 63.5 MiB | 141.7 MiB | 2.23× |
| Request/response · responder | 45.7 MiB | 211.8 MiB | 4.63× |
| Maximum resource segment · initiator | 14.9 MiB | 326.7 MiB | 21.96× |
| Maximum resource segment · responder | 14.0 MiB | 111.7 MiB | 7.95× |
| Maximum resource segment · matched RNS policy · initiator | 13.5 MiB | 325.6 MiB | 24.17× |
| Maximum resource segment · matched RNS policy · responder | 11.2 MiB | 111.3 MiB | 9.98× |
| 64-segment resource stream · initiator | 19.0 MiB | 173.0 MiB | 9.11× |
| 64-segment resource stream · responder | 14.5 MiB | 242.5 MiB | 16.72× |
| 64-segment resource stream · matched RNS policy · initiator | 16.2 MiB | 172.4 MiB | 10.62× |
| 64-segment resource stream · matched RNS policy · responder | 12.1 MiB | 233.8 MiB | 19.31× |
| Raw transport throughput · relay | 48.4 MiB | 171.2 MiB | 3.54× |
| Transported resource throughput · relay | 16.3 MiB | 83.2 MiB | 5.09× |
| Transported resource throughput · matched RNS policy · relay | 7.4 MiB | 83.3 MiB | 11.32× |

A dash means no current three-sample release evidence is published for that scenario.

</details>

## Detailed results

### Links

#### Link-message throughput (v8)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 6951353/6951353 · 3/3 samples | 78.3k/s | 18.79 MB/s | <1.00 / 1.00 ms | i 6.8 MiB / r 50.5 MiB | i 0.12 mJ / r 0.09 mJ |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 767973/767973 · 3/3 samples | 8.5k/s | 2.05 MB/s | 2.00 / 2.00 ms | i 6.3 MiB / r 115.0 MiB | i 0.88 mJ / r 0.61 mJ |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 545055/545055 · 3/3 samples | 6.1k/s | 1.46 MB/s | 3.00 / 3.00 ms | i 113.6 MiB / r 16.2 MiB | i 0.94 mJ / r 0.81 mJ |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 533693/533693 · 3/3 samples | 6.0k/s | 1.43 MB/s | 3.00 / 3.00 ms | i 112.8 MiB / r 103.1 MiB | i 1.08 mJ / r 0.72 mJ |

### Packets

#### Single-packet throughput (v6)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3809336/3809336 · 3/3 samples | 43.8k/s | 9.64 MB/s | <1.00 / 1.00 ms | i 6.6 MiB / r 49.6 MiB | i 0.22 mJ / r 0.15 mJ |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 377453/377453 · 3/3 samples | 4.2k/s | 922.0 kB/s | 4.00 / 4.00 ms | i 6.2 MiB / r 78.8 MiB | i 1.14 mJ / r 1.13 mJ |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 36060/36060 · 3/3 samples | 398/s | 87.5 kB/s | 40.00 / 41.00 ms | i 47.2 MiB / r 6.9 MiB | i 14.87 mJ / r 1.26 mJ |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 35886/35886 · 3/3 samples | 398/s | 87.4 kB/s | 40.00 / 41.00 ms | i 47.3 MiB / r 49.3 MiB | i 15.83 mJ / r 1.55 mJ |

### Requests

#### Request/response (v12)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2760882/2760882 · 3/3 samples | 30.8k/s | 0.13 / 0.15 ms | i 63.5 MiB / r 45.7 MiB | i 0.08 mJ / r 0.09 mJ |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 121353/121353 · 3/3 samples | 1.3k/s | 0.49 / 1.88 ms | i 170.5 MiB / r 8.8 MiB | i 3.15 mJ / r 0.23 mJ |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 100643/100643 · 3/3 samples | 1.1k/s | 1.85 / 9.48 ms | i 141.7 MiB / r 211.8 MiB | i 2.66 mJ / r 4.75 mJ |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 99813/99813 · 3/3 samples | 1.1k/s | 1.21 / 5.84 ms | i 7.9 MiB / r 215.1 MiB | i 0.27 mJ / r 4.77 mJ |

### Resources

#### 64-segment resource stream (v10)

Stream 64 maximum-efficient resource segments with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 911/911 · 3/3 samples | 10/s | 680.01 MB/s | 98.00 / 108.00 ms | i 19.0 MiB / r 14.5 MiB | i 7.00 mJ/MiB / r 7.78 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 179/179 · 3/3 samples | 2/s | 135.36 MB/s | 466.00 / 770.00 ms | i 20.8 MiB / r 258.8 MiB | i 9.75 mJ/MiB / r 32.61 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 167/167 · 3/3 samples | 2/s | 123.44 MB/s | 508.00 / 808.00 ms | i 173.0 MiB / r 242.5 MiB | i 17.37 mJ/MiB / r 31.01 mJ/MiB |
| RNS 1.5.4 → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.25/s | 16.72 MB/s | 4003.00 / 4178.00 ms | i 133.2 MiB / r 10.4 MiB | i 4.03 mJ/MiB / r 2.86 mJ/MiB |

**Cell context**

1. **RNS 1.5.4 → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.5.4's timing-sensitive handoff to a 65th 64-byte tail segment.

#### 64-segment resource stream · matched RNS policy (v1)

Stream 64 maximum-efficient resource segments with both endpoint interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 942/942 · 3/3 samples | 10/s | 699.66 MB/s | 95.00 / 107.00 ms | i 16.2 MiB / r 12.1 MiB | i 7.06 mJ/MiB / r 9.69 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 182/182 · 3/3 samples | 2/s | 136.02 MB/s | 466.00 / 757.00 ms | i 20.8 MiB / r 247.1 MiB | i 9.63 mJ/MiB / r 31.90 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 168/168 · 3/3 samples | 2/s | 125.41 MB/s | 513.00 / 812.00 ms | i 172.4 MiB / r 233.8 MiB | i 17.39 mJ/MiB / r 30.57 mJ/MiB |
| RNS 1.5.4 → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.25/s | 16.75 MB/s | 4016.00 / 4053.00 ms | i 133.2 MiB / r 10.4 MiB | i 3.97 mJ/MiB / r 2.67 mJ/MiB |

**Cell context**

1. **RNS 1.5.4 → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled policy comparison: identical workload and protocol with both implementations configured for the stock RNS bitrate and MTU tier.

> Both implementations carry the same 67,108,800-byte payload in 64 maximum-efficient protocol segments. This is 64 bytes below 64 MiB and avoids making benchmark completion depend on RNS 1.5.4's timing-sensitive handoff to a 65th 64-byte tail segment.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 28334/28334 · 3/3 samples | 315/s | 330.36 MB/s | 3.00 / 3.00 ms | i 14.9 MiB / r 14.0 MiB | i 8.74 mJ/MiB / r 10.07 mJ/MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 20183/20183 · 3/3 samples | 224/s | 234.88 MB/s | 4.00 / 5.00 ms | i 697.3 MiB / r 10.5 MiB | i 16.34 mJ/MiB / r 14.07 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 9755/9755 · 3/3 samples | 108/s | 113.25 MB/s | 8.00 / 9.00 ms | i 12.3 MiB / r 114.8 MiB | i 10.18 mJ/MiB / r 36.81 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 8558/8558 · 3/3 samples | 95/s | 99.98 MB/s | 10.00 / 11.00 ms | i 326.7 MiB / r 111.7 MiB | i 19.03 mJ/MiB / r 37.75 mJ/MiB |

#### Maximum resource segment · matched RNS policy (v1)

Repeat maximum-efficient resource transfers with both endpoint interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 28694/28694 · 3/3 samples | 320/s | 336.05 MB/s | 3.00 / 3.00 ms | i 13.5 MiB / r 11.2 MiB | i 8.25 mJ/MiB / r 11.77 mJ/MiB |
| RNS 1.5.4 → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 20198/20198 · 3/3 samples | 224/s | 234.99 MB/s | 4.00 / 5.00 ms | i 695.0 MiB / r 10.5 MiB | i 16.08 mJ/MiB / r 14.09 mJ/MiB |
| Prns → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 9776/9776 · 3/3 samples | 109/s | 114.02 MB/s | 8.00 / 10.00 ms | i 12.4 MiB / r 117.5 MiB | i 10.15 mJ/MiB / r 37.02 mJ/MiB |
| RNS 1.5.4 → RNS 1.5.4 | <img src="assets/check.svg" width="14" alt="conformant" /> 8585/8585 · 3/3 samples | 96/s | 100.37 MB/s | 10.00 / 11.00 ms | i 325.6 MiB / r 111.3 MiB | i 18.95 mJ/MiB / r 36.99 mJ/MiB |

> Controlled policy comparison: identical workload and protocol with both implementations configured for the stock RNS bitrate and MTU tier.

### Transport

#### Raw transport throughput (v2)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 91159211/91159211 · 3/3 samples | 242.97 MB/s | 1.01M/s | 352.93 MB/s / 336.74 MB/s | 33.81 s | 48.4 MiB | 6.34× / 10.18× / 6.34× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 3601385/3601385 · 3/3 samples | 9.59 MB/s | 40.0k/s | 13.93 MB/s / 13.29 MB/s | 35.96 s | 171.2 MiB | 160.73× / 261.64× / 160.73× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal host-interface policy; the exact bitrate and MTU are reported with every row.

#### Transported resource throughput (v2)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default host-interface policy.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | 128 / 128 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 2307606/2307606 · 3/3 samples | 3318.78 MB/s | 25.3k/s | 3345.52 MB/s / 3345.52 MB/s | 34.72 s | 16.3 MiB | 3.68× / 3.07× / 3.07× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 868832/868832 · 3/3 samples | 156.76 MB/s | 9.6k/s | 158.19 MB/s / 158.19 MB/s | 34.67 s | 83.2 MiB | 44.35× / 68.70× / 44.35× |

> Default-policy deployment view: Prns and RNS retain their normal host-interface bitrate and MTU policy.

#### Transported resource throughput · matched RNS policy (v1)

Relay the identical transported-resource workload with both relay interfaces configured for RNS's 10 Mbps / 16 KiB policy.

| Relay | Interface policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 15465077/15465077 · 3/3 samples | 2781.48 MB/s | 170.0k/s | 2806.93 MB/s / 2806.93 MB/s | 37.94 s | 7.4 MiB | 2.47× / 3.77× / 2.47× |
| RNS 1.5.4 relay | 10 Mbps / 16 KiB | 16 / 16 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 866475/866475 · 3/3 samples | 156.86 MB/s | 9.6k/s | 158.29 MB/s / 158.29 MB/s | 34.57 s | 83.3 MiB | 43.08× / 68.82× / 43.08× |

> Controlled policy comparison: identical transported link and driver with both implementations configured for the stock RNS bitrate and MTU tier.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.5.4** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, actual HDLC-framed TCP wire rates, relay-only CPU/RSS, and full-path driver source/sink/limiting headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
