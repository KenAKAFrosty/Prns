# Benchmark results — `aarch64-apple-darwin`

[← All hosts](RESULTS.md)

## Machine and method

Apple M4; 10 physical / 10 logical; 16.0 GiB; macOS 26.4.

Release binaries run over loopback for 30 seconds per sample, three samples per cell. Tables show median throughput and latency; memory is the maximum peak RSS. Energy is optional: it is metered processor energy minus a fresh idle baseline (macOS CPU Power; Linux RAPL package) and appears only when all three samples are positive. Packet/request energy is per delivery; resource energy is normalized per application MiB. Initiator/responder energy is the combined package measurement attributed by each role's CPU-time share. A check means every sample satisfied the scenario's accounting rule.

## At a glance

| Scenario | Prns | Reference | Prns / reference |
|---|---:|---:|---:|
| Single-packet throughput | 39.7k/s | 1.9k/s | 20.39× |
| Link-message throughput | — | — | — |
| Request/response | — | — | — |
| Maximum resource segment | 274.30 MB/s | 99.17 MB/s | 2.77× |
| 64 MiB resource stream | 459.25 MB/s | 121.37 MB/s | 3.78× |

A dash means no current three-sample release evidence is published for that scenario.

## Detailed results

### Packets

#### Single-packet throughput (v2)

Sustained proved delivery of varied-size one-shot packets over TCP.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / delivery (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 3576046/3576046 · 3/3 samples | 39.7k/s | 8.73 MB/s | <1.00 / 1.00 ms | i 16.8 MiB / r 48.7 MiB | i 0.24 mJ / r 0.18 mJ |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 373203/373203 · 3/3 samples | 4.2k/s | 918.8 kB/s | 4.00 / 4.00 ms | i 5.5 MiB / r 230.0 MiB | i 1.47 mJ / r 1.20 mJ |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 151430/151430 · 3/3 samples | 1.9k/s | 428.7 kB/s | 1.00 / 17.00 ms | i 202.2 MiB / r 208.4 MiB | i 3.08 mJ / r 1.43 mJ |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 168054/168054 · 3/3 samples | 1.9k/s | 420.0 kB/s | <1.00 / 19.00 ms | i 203.1 MiB / r 8.0 MiB | i 2.91 mJ / r 1.62 mJ |

### Resources

#### 64 MiB resource stream (v2)

Stream a 64 MiB incompressible resource with compression disabled.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 618/618 · 3/3 samples | 7/s | 459.25 MB/s | 145.00 / 163.00 ms | i 48.5 MiB / r 40.4 MiB | i 8.00 mJ/MiB / r 7.60 mJ/MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 180/180 · 3/3 samples | 2/s | 133.31 MB/s | 491.00 / 630.00 ms | i 18.1 MiB / r 416.3 MiB | i 9.63 mJ/MiB / r 32.61 mJ/MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 164/164 · 3/3 samples | 2/s | 121.37 MB/s | 542.00 / 612.00 ms | i 344.2 MiB / r 411.4 MiB | i 21.32 mJ/MiB / r 33.15 mJ/MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 21/21 · 3/3 samples | 0.22/s | 15.02 MB/s | 4349.00 / 5129.00 ms | i 279.1 MiB / r 9.5 MiB | i 13.76 mJ/MiB / r 4.82 mJ/MiB |

#### Maximum resource segment (v1)

Repeated transfer of one maximum-efficient resource segment.

| Subject | Conformance | Rate | Goodput | RTT p50 / p99 | Peak RSS (i / r) | Energy / MiB (i / r) |
|---|---:|---:|---:|---:|---:|---:|
| Prns → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 23544/23544 · 3/3 samples | 262/s | 274.30 MB/s | 3.00 / 4.00 ms | i 40.5 MiB / r 40.0 MiB | i 7.95 mJ/MiB / r 7.85 mJ/MiB |
| RNS 1.4.0 (compiled) → Prns | <img src="assets/check.svg" width="14" alt="conformant" /> 15341/15341 · 3/3 samples | 170/s | 178.50 MB/s | 6.00 / 6.00 ms | i 727.4 MiB / r 10.0 MiB | i 20.37 mJ/MiB / r 11.05 mJ/MiB |
| Prns → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 9525/9525 · 3/3 samples | 108/s | 112.76 MB/s | 9.00 / 12.00 ms | i 11.0 MiB / r 301.5 MiB | i 8.81 mJ/MiB / r 31.54 mJ/MiB |
| RNS 1.4.0 (compiled) → RNS 1.4.0 (compiled) | <img src="assets/check.svg" width="14" alt="conformant" /> 8501/8501 · 3/3 samples | 95/s | 99.17 MB/s | 11.00 / 12.00 ms | i 503.7 MiB / r 289.7 MiB | i 21.39 mJ/MiB / r 35.79 mJ/MiB |

## Implementation legend

- **Prns** — Rust, ed25519-dalek 2.2.

- **RNS 1.4.0 (compiled)** — Python, PyCA cryptography / OpenSSL; reference.

## Metric legend

Conformance is clean samples and exact delivered/sent accounting. Rows are ordered by median throughput, never by memory or energy. Rate is median settled operations per second. Goodput is median application bytes per second. RTT is median p50/p99 settlement latency. Peak RSS shows the largest initiator (`i`) and responder (`r`) process peaks across samples. Energy shows optional initiator/responder attribution of median net processor energy and appears only with three positive-baseline samples: per delivery for packets/requests, per application MiB for resources.
