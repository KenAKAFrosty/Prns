# Benchmark results — `x86_64-pc-windows-msvc`

[← All hosts](RESULTS.md)

> **Qualification: COMPLETE.** 20/20 cells; 60/60 conformant samples; exact source `274052c22149ff250e85ca66b776c15515c7c560`; source tree clean.

## Machine and method

AMD Ryzen 5 5600X 6-Core Processor; 6 physical / 12 logical; 31.9 GiB; Windows 11 Home.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. A check means every sample satisfied the scenario's accounting rule.

## At a glance

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 25.6k/s | 3.4k/s | 7.46× |
| Link-message throughput | 38.0k/s | 4.1k/s | 9.30× |
| Request/response | 4.4k/s | 294/s | 15.05× |
| Maximum resource segment | 124.66 MB/s | 39.85 MB/s | 3.13× |
| 64 MiB resource stream | 110.47 MB/s | 50.55 MB/s | 2.19× |

A dash means no current three-sample release evidence is published for that scenario.

## Detailed results

### Links

#### Link-message throughput (v6)

Sustained delivery of small messages over one established link.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3415275/3415275 · 3/3 samples | 38.0k/s | 9.11 MB/s | <1.00 / 1.00 ms | i 25.5 MiB / r 47.4 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 376839/376839 · 3/3 samples | 4.2k/s | 1.00 MB/s | 4.00 / 4.00 ms | i 10.3 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 367216/367216 · 3/3 samples | 4.1k/s | 979.4 kB/s | 4.00 / 4.00 ms | i 5.7 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 86519/86519 · 3/3 samples | 961/s | 231.0 kB/s | <1.00 / <1.00 ms | i 5.7 MiB / r 9.7 MiB |

### Packets

#### Single-packet throughput (v4)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 2297481/2297481 · 3/3 samples | 25.6k/s | 5.63 MB/s | 1.00 / 1.00 ms | i 17.4 MiB / r 46.5 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 593074/593074 · 3/3 samples | 6.6k/s | 1.45 MB/s | 2.00 / 3.00 ms | i 11.3 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 308097/308097 · 3/3 samples | 3.4k/s | 753.5 kB/s | <1.00 / <1.00 ms | i 5.7 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 306705/306705 · 3/3 samples | 3.4k/s | 748.8 kB/s | <1.00 / <1.00 ms | i 5.7 MiB / r 14.0 MiB |

### Requests

#### Request/response (v11)

Four concurrent small requests with asynchronous 1–4 KiB resource responses over four pre-established links.

| Subject | Conformance | Rate | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 397850/397850 · 3/3 samples | 4.4k/s | 0.91 / 1.30 ms | i 18.9 MiB / r 19.7 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 49348/49348 · 3/3 samples | 552/s | 1.84 / 254.32 ms | i 10.1 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 32727/32727 · 3/3 samples | 358/s | 9.36 / 43.05 ms | i 5.7 MiB / r 10.9 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 26959/26959 · 3/3 samples | 294/s | 9.98 / 117.74 ms | i 5.7 MiB / r 5.7 MiB |

### Resources

#### 64 MiB resource stream (v8)

Stream a 64 MiB incompressible resource with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 146/146 · 3/3 samples | 2/s | 110.47 MB/s | 611.00 / 751.00 ms | i 47.4 MiB / r 48.8 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 93/93 · 3/3 samples | 1/s | 68.47 MB/s | 1028.00 / 1298.00 ms | i 17.2 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 69/69 · 3/3 samples | 0.75/s | 50.55 MB/s | 1265.00 / 1552.00 ms | i 5.7 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 24/24 · 3/3 samples | 0.25/s | 16.61 MB/s | 3998.00 / 4216.00 ms | i 5.7 MiB / r 12.6 MiB |

#### Maximum resource segment (v7)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) |
|---|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 10672/10672 · 3/3 samples | 119/s | 124.66 MB/s | 6.00 / 26.00 ms | i 44.5 MiB / r 49.0 MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 7756/7756 · 3/3 samples | 86/s | 90.07 MB/s | 11.00 / 13.00 ms | i 5.7 MiB / r 13.4 MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 4191/4191 · 3/3 samples | 47/s | 48.95 MB/s | 14.00 / 17.00 ms | i 13.8 MiB / r 5.7 MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 3419/3419 · 3/3 samples | 38/s | 39.85 MB/s | 19.00 / 21.00 ms | i 5.7 MiB / r 5.7 MiB |

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.0 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources.
