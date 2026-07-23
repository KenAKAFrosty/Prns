# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 34/34 cells; 102/102 conformant samples; exact source `ab5ff5ab0c5591dbe570f96001c590b29c841a1a`; source tree clean.

## Machine and method

Apple M4; 10 physical / 10 logical; 16.0 GiB; macOS 26.4.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Endpoint scenarios cover all four initiator/responder pairings; relay scenarios cover both implementations behind the same fixed bidirectional wire driver. Default-policy rows preserve each implementation's normal TCP bitrate and MTU policy. The controlled 1 Gbps resource rows change only that interface policy, both for real endpoint transfers and for transported-resource switching; the tiny raw SINGLE relay scenario remains default-policy-only. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. Relay-scenario package energy is explicitly whole-cell energy; only CPU and RSS are relay-isolated. A check means every sample satisfied the scenario's accounting rule.

## At a glance

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 39.6k/s | 1.8k/s | 22.60× |
| Link-message throughput | 75.6k/s | 4.7k/s | 16.15× |
| Request/response | 6.3k/s | 1.1k/s | 5.98× |
| Maximum resource segment | 302.23 MB/s | 96.66 MB/s | 3.13× |
| Maximum resource segment · 1 Gbps policy | 301.36 MB/s | 91.87 MB/s | 3.28× |
| 64 MiB resource stream | 461.93 MB/s | 120.49 MB/s | 3.83× |
| 64 MiB resource stream · 1 Gbps policy | 459.79 MB/s | 110.96 MB/s | 4.14× |
| Raw transport throughput | 106.00 MB/s | 11.07 MB/s | 9.57× |
| Transported resource throughput | 1290.36 MB/s | 146.28 MB/s | 8.82× |
| Transported resource throughput · 1 Gbps policy | 1201.08 MB/s | 155.61 MB/s | 7.72× |

A dash means no current three-sample release evidence is published for that scenario.

## Detailed results

### Links

#### Link-message throughput (v6)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 6811247/6811247 · 3/3 samples | 75.6k/s | 18.16 MB/s | <1.00 / 1.00 ms | i 25.2 MiB / r 48.3 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 776000/776000 · 3/3 samples | 8.6k/s | 2.07 MB/s | 2.00 / 2.00 ms | i 7.5 MiB / r 262.2 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 427865/427865 · 3/3 samples | 4.7k/s | 1.12 MB/s | 1.00 / 2.00 ms | i 234.4 MiB / r 232.0 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 295616/295616 · 3/3 samples | 3.3k/s | 781.0 kB/s | <1.00 / 1.00 ms | i 226.1 MiB / r 10.7 MiB |

### Packets

#### Single-packet throughput (v4)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3556726/3556726 · 3/3 samples | 39.6k/s | 8.70 MB/s | <1.00 / 1.00 ms | i 16.5 MiB / r 48.6 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 371891/371891 · 3/3 samples | 4.1k/s | 908.1 kB/s | 4.00 / 4.00 ms | i 5.3 MiB / r 225.4 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 169468/169468 · 3/3 samples | 1.9k/s | 418.5 kB/s | <1.00 / 20.00 ms | i 201.1 MiB / r 7.5 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 160178/160178 · 3/3 samples | 1.8k/s | 385.3 kB/s | 1.00 / 20.00 ms | i 203.8 MiB / r 206.5 MiB |

### Requests

#### Request/response (v11)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 534293/534293 · 3/3 samples | 6.3k/s | 0.64 / 0.85 ms | i 17.8 MiB / r 45.2 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 191080/191080 · 3/3 samples | 2.1k/s | 1.62 / 3.78 ms | i 334.6 MiB / r 37.5 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 95147/95147 · 3/3 samples | 1.1k/s | 1.48 / 14.69 ms | i 288.3 MiB / r 341.3 MiB |
| Prns → RNS 1.4.0 (compiled)<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 90149/90149 · 3/3 samples | 982/s | 1.11 / 251.69 ms | i 7.6 MiB / r 346.0 MiB |

**Cell context**

1. **Prns → RNS 1.4.0 (compiled)** — RNS sends a resource advertisement before registering that resource internally. Prns can return the first pull so quickly that RNS drops it. Prns then waits for its 250 ms retry deadline. The published 251.69 ms p99 is the fingerprint of this race.

### Resources

#### 64 MiB resource stream (v9)

Stream a 64 MiB incompressible resource with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 620/620 · 3/3 samples | 7/s | 461.93 MB/s | 144.00 / 160.00 ms | i 49.2 MiB / r 40.3 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 179/179 · 3/3 samples | 2/s | 132.79 MB/s | 494.00 / 552.00 ms | i 17.2 MiB / r 415.1 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 162/162 · 3/3 samples | 2/s | 120.49 MB/s | 546.00 / 613.00 ms | i 408.4 MiB / r 397.6 MiB |
| RNS 1.4.0 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.24/s | 16.10 MB/s | 4199.00 / 4541.00 ms | i 341.4 MiB / r 9.3 MiB |

**Cell context**

1. **RNS 1.4.0 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Both implementations carry the same 64 MiB in 65 authenticated protocol segments. RNS 1.4.0 fills 64 maximum-efficient segments and ends with a 64-byte tail; Prns keeps the first 63 at that ceiling and balances the final pair. The protocol-valid rebalance avoids an RNS 1.4.0 receive-side handoff race in which the proof leaves before the retiring receiver is removed and that one untagged tail part can be skipped.

#### 64 MiB resource stream · 1 Gbps policy (v2)

Stream the 64 MiB resource with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 618/618 · 3/3 samples | 7/s | 459.79 MB/s | 145.00 / 163.00 ms | i 145.6 MiB / r 142.9 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 163/163 · 3/3 samples | 2/s | 120.62 MB/s | 547.00 / 583.00 ms | i 145.6 MiB / r 554.7 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 148/148 · 3/3 samples | 2/s | 110.96 MB/s | 598.00 / 635.00 ms | i 346.9 MiB / r 532.0 MiB |
| RNS 1.4.0 (compiled) → Prns<sup>1</sup> | <img src="assets/check.svg" width="14" alt="conformant" /> 23/23 · 3/3 samples | 0.24/s | 15.97 MB/s | 4086.00 / 4902.00 ms | i 344.8 MiB / r 143.2 MiB |

**Cell context**

1. **RNS 1.4.0 (compiled) → Prns** — RNS prepares the next 1 MiB segment in a background thread. Prns proves the current segment before that preparation completes, making RNS enter a coarse 50 ms polling loop. A slower stock receiver gives preparation enough time to finish, avoiding that cliff.

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

> Both implementations carry the same 64 MiB in 65 authenticated protocol segments. RNS 1.4.0 fills 64 maximum-efficient segments and ends with a 64-byte tail; Prns keeps the first 63 at that ceiling and balances the final pair. The protocol-valid rebalance avoids an RNS 1.4.0 receive-side handoff race in which the proof leaves before the retiring receiver is removed and that one untagged tail part can be skipped.

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 25977/25977 · 3/3 samples | 288/s | 302.23 MB/s | 3.00 / 3.00 ms | i 43.7 MiB / r 39.9 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15206/15206 · 3/3 samples | 169/s | 177.21 MB/s | 6.00 / 6.00 ms | i 711.6 MiB / r 9.8 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 9840/9840 · 3/3 samples | 110/s | 114.85 MB/s | 8.00 / 9.00 ms | i 11.2 MiB / r 296.2 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 8288/8288 · 3/3 samples | 92/s | 96.66 MB/s | 11.00 / 12.00 ms | i 493.3 MiB / r 286.9 MiB |

#### Maximum resource segment · 1 Gbps policy (v1)

Repeat maximum-efficient resource transfers with both endpoint TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 25899/25899 · 3/3 samples | 287/s | 301.36 MB/s | 3.00 / 3.00 ms | i 74.6 MiB / r 76.1 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 17856/17856 · 3/3 samples | 201/s | 210.42 MB/s | 5.00 / 5.00 ms | i 663.6 MiB / r 109.0 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 8968/8968 · 3/3 samples | 100/s | 104.92 MB/s | 9.00 / 11.00 ms | i 73.9 MiB / r 321.7 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 7875/7875 · 3/3 samples | 88/s | 91.87 MB/s | 11.00 / 13.00 ms | i 402.5 MiB / r 304.1 MiB |

> Controlled computational comparison: identical workload and protocol, with only TCP bitrate policy changed.

### Transport

#### Raw transport throughput (v1)

Balanced bidirectional switching of opaque packets through a pure transport node.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness headroom |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 39590257/39590257 · 3/3 samples | 106.00 MB/s | 441.7k/s | 154.20 MB/s / 147.14 MB/s | 42.29 s | 48.9 MiB | 1.51× |
| RNS 1.4.0 (compiled) relay | 10 Mbps / 8 KiB | — | <img src="assets/check.svg" width="14" alt="conformant" /> 4183453/4183453 · 3/3 samples | 11.07 MB/s | 46.1k/s | 16.11 MB/s / 15.37 MB/s | 36.82 s | 320.1 MiB | 14.54× |

> Announce signing and verification happen before measurement; the timed path switches opaque transport data.

> This practical profile preserves each implementation's normal TCP policy: 500 Mbps for Prns and 10 Mbps for compiled RNS 1.4.0.

#### Transported resource throughput (v1)

Relay balanced near-MTU resource parts over one warm transported link using each implementation's default TCP policy.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness headroom |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 500 Mbps / 128 KiB | 128 / 128 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 891152/891152 · 3/3 samples | 1290.36 MB/s | 9.8k/s | 1300.76 MB/s / 1300.76 MB/s | 33.04 s | 146.7 MiB | 7.91× |
| RNS 1.4.0 (compiled) relay | 10 Mbps / 8 KiB | 8 / 8 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 1616021/1616021 · 3/3 samples | 146.28 MB/s | 17.9k/s | 147.71 MB/s / 147.71 MB/s | 32.34 s | 264.0 MiB | 27.42× |

> Default-policy deployment view: Prns and RNS retain their normal TCP bitrate and MTU policy.

#### Transported resource throughput · 1 Gbps policy (v1)

Relay the identical transported-resource workload with both relay TCP interfaces explicitly configured for the 1 Gbps MTU tier.

| Relay | TCP policy / MTU | Link MTU / payload | Conformance | Payload | Frames | Wire in / out | Relay CPU | Relay peak RSS | Harness headroom |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Prns relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 206986/206986 · 3/3 samples | 1201.08 MB/s | 2.3k/s | 1210.58 MB/s / 1210.58 MB/s | 32.77 s | 538.4 MiB | 9.56× |
| RNS 1.4.0 (compiled) relay | 1 Gbps / 512 KiB | 512 / 512 KiB | <img src="assets/check.svg" width="14" alt="conformant" /> 26788/26788 · 3/3 samples | 155.61 MB/s | 297/s | 156.84 MB/s / 156.84 MB/s | 33.42 s | 208.8 MiB | 74.33× |

> Controlled computational comparison: identical transported link and driver, with only TCP bitrate policy changed.

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.0 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. Relay scenarios report carried opaque payload bytes, forwarded frames, HDLC-framed TCP wire rates, relay-only CPU/RSS, and direct-driver headroom; transported-resource rows additionally expose negotiated link MTU and payload bytes per part. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources. Relay-scenario energy is whole-cell package energy, never relay-only energy.
