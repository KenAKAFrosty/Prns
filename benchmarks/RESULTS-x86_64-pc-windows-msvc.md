# Benchmark results — `x86_64-pc-windows-msvc`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `a7b97c7403a56c0fbcd35f5befa9ee1049ce4d33`; source tree clean.

## Machine and method

AMD Ryzen 5 5600X 6-Core Processor; 6 physical / 12 logical; 31.9 GiB; Windows 11 Home.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Default-policy rows preserve each implementation's normal TCP bitrate and MTU policy. The controlled 1 Gbps resource rows change only that interface policy, both for real endpoint transfers and for transported-resource switching; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-x86_64-pc-windows-msvc-dark.svg">
  <img alt="Bar chart of Prns median throughput as a multiple of RNS 1.4.0 (compiled) for each published scenario" src="assets/at-a-glance-x86_64-pc-windows-msvc-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/at-a-glance-memory-x86_64-pc-windows-msvc-dark.svg">
  <img alt="Bar chart of RNS 1.4.0 (compiled) peak memory as a multiple of Prns for each role and scenario" src="assets/at-a-glance-memory-x86_64-pc-windows-msvc-light.svg">
</picture>

<details>
<summary>Chart data as a table</summary>

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 25.5k/s | 3.4k/s | 7.60× |
| Link-message throughput | 37.9k/s | 4.1k/s | 9.24× |
| Request/response | 4.4k/s | 284/s | 15.62× |
| Maximum resource segment | 108.70 MB/s | 38.81 MB/s | 2.80× |
| Maximum resource segment · 1 Gbps policy | 184.44 MB/s | 23.11 MB/s | 7.98× |
| 64 MiB resource stream | 131.72 MB/s | 46.80 MB/s | 2.81× |
| 64 MiB resource stream · 1 Gbps policy | 264.89 MB/s | 36.50 MB/s | 7.26× |
| Raw transport throughput | 148.67 MB/s | 3.78 MB/s | 39.33× |
| Transported resource throughput | 161.87 MB/s | 123.44 MB/s | 1.31× |
| Transported resource throughput · 1 Gbps policy | 470.13 MB/s | 94.84 MB/s | 4.96× |

A dash means no current three-sample release evidence is published for that scenario.

</details>

## Detailed results

### Links

#### Link-message throughput (v8)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3410264/3410264 · 3/3 samples | 37.9k/s | 9.09 MB/s | <1.00 / 1.00 ms | i 10.0 MiB / r 47.7 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 377455/377455 · 3/3 samples | 4.2k/s | 1.00 MB/s | <1.00 / <1.00 ms | i 225.8 MiB / r 13.8 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 374226/374226 · 3/3 samples | 4.1k/s | 991.7 kB/s | 4.00 / 4.00 ms | i 9.9 MiB / r 218.5 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 368737/368737 · 3/3 samples | 4.1k/s | 983.2 kB/s | 4.00 / 4.00 ms | i 225.9 MiB / r 218.3 MiB |

### Packets

#### Single-packet throughput (v6)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2290561/2290561 · 3/3 samples | 25.5k/s | 5.61 MB/s | 1.00 / 1.00 ms | i 8.8 MiB / r 45.6 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 573259/573259 · 3/3 samples | 6.5k/s | 1.42 MB/s | 2.00 / 3.00 ms | i 11.4 MiB / r 238.9 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 300728/300728 · 3/3 samples | 3.4k/s | 738.0 kB/s | <1.00 / <1.00 ms | i 209.1 MiB / r 214.5 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 300477/300477 · 3/3 samples | 3.3k/s | 733.2 kB/s | <1.00 / <1.00 ms | i 209.1 MiB / r 16.8 MiB |

### Requests

#### Request/response (v12)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 398838/398838 · 3/3 samples | 4.4k/s | 0.91 / 1.31 ms | i 19.5 MiB / r 19.9 MiB |
| Prns → RNS 1.4.0 (compiled)<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 46313/46313 · 3/3 samples | 508/s | 1.84 / 254.71 ms | i 9.5 MiB / r 262.1 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 31182/31182 · 3/3 samples | 346/s | 9.36 / 48.10 ms | i 218.2 MiB / r 11.1 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 25798/25798 · 3/3 samples | 284/s | 9.92 / 259.31 ms | i 215.8 MiB / r 226.7 MiB |

**Cell context**

1. **Prns → RNS 1.4.0 (compiled)** — RNS sends a resource advertisement before registering that resource internally. Prns can return the first pull so quickly that RNS drops it. Prns then waits for its 250 ms retry deadline. A p99 pinned just above 250 ms is the fingerprint of this race.

### Resources

#### 64 MiB resource stream (v9)

Stream a 64 MiB incompressible resource with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 178/178 · 3/3 samples | 2/s | 131.72 MB/s | 506.00 / 613.00 ms | i 47.5 MiB / r 48.7 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 105/105 · 3/3 samples | 1/s | 76.56 MB/s | 805.00 / 1131.00 ms | i 17.3 MiB / r 363.4 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 64/64 · 3/3 samples | 0.70/s | 46.80 MB/s | 1272.00 / 2092.00 ms | i 321.2 MiB / r 357.6 MiB |
| RNS 1.4.0 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.25/s | 16.62 MB/s | 4028.00 / 4088.00 ms | i 321.0 MiB / r 12.9 MiB |

**Cell context**

1. **RNS 1.4.0 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 64 MiB in 65 authenticated protocol segments. RNS 1.4.0 fills 64 maximum-efficient segments and ends with a 64-byte tail; Prns keeps the first 63 at that ceiling and balances the final pair. The protocol-valid rebalance avoids an RNS 1.4.0 receive-side handoff race in which the proof leaves before the retiring receiver is removed and that one untagged tail part can be skipped.

#### 64 MiB resource stream · 1 Gbps policy (v2)

Stream the 64 MiB resource with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 356/356 · 3/3 samples | 4/s | 264.89 MB/s | 251.00 / 271.00 ms | i 144.6 MiB / r 150.3 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 81/81 · 3/3 samples | 0.84/s | 56.42 MB/s | 973.00 / 5615.00 ms | i 144.6 MiB / r 424.4 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 48/48 · 3/3 samples | 0.54/s | 36.50 MB/s | 1438.00 / 6723.00 ms | i 321.9 MiB / r 406.7 MiB |
| RNS 1.4.0 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.26/s | 17.34 MB/s | 3870.00 / 3913.00 ms | i 321.9 MiB / r 150.9 MiB |

**Cell context**

1. **RNS 1.4.0 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

> Both implementations carry the same 64 MiB in 65 authenticated protocol segments. RNS 1.4.0 fills 64 maximum-efficient segments and ends with a 64-byte tail; Prns keeps the first 63 at that ceiling and balances the final pair. The protocol-valid rebalance avoids an RNS 1.4.0 receive-side handoff race in which the proof leaves before the retiring receiver is removed and that one untagged tail part can be skipped.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 9583/9583 · 3/3 samples | 104/s | 108.70 MB/s | 6.00 / 26.00 ms | i 45.0 MiB / r 49.3 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 7613/7613 · 3/3 samples | 85/s | 89.24 MB/s | 12.00 / 13.00 ms | i 434.2 MiB / r 13.3 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 4175/4175 · 3/3 samples | 46/s | 48.57 MB/s | 14.00 / 23.00 ms | i 14.4 MiB / r 233.3 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 3313/3313 · 3/3 samples | 37/s | 38.81 MB/s | 19.00 / 34.00 ms | i 306.5 MiB / r 223.6 MiB |

#### Maximum resource segment · 1 Gbps policy (v1)

Repeat maximum-efficient resource transfers with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15815/15815 · 3/3 samples | 176/s | 184.44 MB/s | 5.00 / 6.00 ms | i 78.4 MiB / r 81.7 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 8101/8101 · 3/3 samples | 91/s | 95.48 MB/s | 11.00 / 12.00 ms | i 398.0 MiB / r 115.7 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 3409/3409 · 3/3 samples | 38/s | 39.84 MB/s | 17.00 / 37.00 ms | i 77.9 MiB / r 243.2 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 1981/1981 · 3/3 samples | 22/s | 23.11 MB/s | 40.00 / 43.00 ms | i 257.7 MiB / r 227.1 MiB |

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

### Transport

#### Raw transport throughput (v2)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 55903237/55903237 · 3/3 samples | 148.67 MB/s | 619.5k/s | 215.97 MB/s / 206.06 MB/s | 35.94 s | 48.5 MiB | 10.41× / 5.41× / 5.41× |
| RNS 1.4.0 (compiled) relay | 10 Mbps / 8 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 1412195/1412195 · 3/3 samples | 3.78 MB/s | 15.7k/s | 5.49 MB/s / 5.24 MB/s | 46.78 s | 295.4 MiB | 410.49× / 210.31× / 210.31× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal TCP policy: 500 Mbps for Prns and 10 Mbps for compiled RNS 1.4.0.

#### Transported resource throughput (v2)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default TCP policy.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | 128 / 128 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 111588/111588 · 3/3 samples | 161.87 MB/s | 1.2k/s | 163.17 MB/s / 163.17 MB/s | 6.22 s | 148.3 MiB | 31.98× / 5.31× / 5.31× |
| RNS 1.4.0 (compiled) relay | 10 Mbps / 8 KiB | 8 / 8 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 1360549/1360549 · 3/3 samples | 123.44 MB/s | 15.1k/s | 124.64 MB/s / 124.64 MB/s | 44.22 s | 242.6 MiB | 9.45× / 39.59× / 9.45× |

> Default-policy deployment view: Prns and RNS retain their normal TCP bitrate and MTU policy.

#### Transported resource throughput · 1 Gbps policy (v2)

Relay the identical transported-resource workload with both relay TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness source / sink / limit |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 79457/79457 · 3/3 samples | 470.13 MB/s | 897/s | 473.85 MB/s / 473.85 MB/s | 20.88 s | 538.9 MiB | 9.06× / 5.21× / 5.21× |
| RNS 1.4.0 (compiled) relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 16334/16334 · 3/3 samples | 94.84 MB/s | 181/s | 95.59 MB/s / 95.59 MB/s | 34.83 s | 198.8 MiB | 42.70× / 25.66× / 25.66× |

> Controlled computational comparison: identical transported link and driver, with only TCP bitrate policy changed.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.0 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, actual HDLC-framed TCP wire rates, relay-only CPU/RSS, and full-path driver source/sink/limiting headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
