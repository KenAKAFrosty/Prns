# Benchmarking Prns

The benchmark harness compares Prns and the locked reference implementation
under identical scenario-owned workloads. It records conformance, throughput,
latency, CPU, memory, and optional energy evidence. A smoke run checks the
machinery; only a complete qualified publication supports a release claim.

## Published performance summary

The current [published suites](RESULTS.md) compare Prns with **stock interpreted
RNS 1.5.4**, using PyCA cryptography / OpenSSL. Each suite records
`reference_proof.compiled = false` and zero native RNS modules. Older compiled
RNS results remain historical evidence; they are not included in these claims.

| Claim | Published comparison and scope |
|---|---|
| Up to 110× throughput | **110.22×**, single-packet delivery on the [Apple M4](RESULTS-aarch64-apple-darwin.md). |
| Middle half roughly 5×–25× | **5.42×–24.29×**, across the 30 host/scenario throughput ratios in the three at-a-glance tables; median **9.62×**. |
| Full throughput range | **1.02×–110.22×**. The low end is default-policy transported resources on [Windows](RESULTS-x86_64-pc-windows-msvc.md); resource-segment rows on Linux and macOS are around 3×. |
| Peak-memory example | **About 97% less peak memory than stock RNS**, measured as initiator peak RSS of the matched-policy 64-segment stream on [Linux](RESULTS-x86_64-unknown-linux-gnu.md). |
| Processor-energy example | **About 98% less processor energy per request than stock RNS** for Linux request/response: stock **31.2043 mJ/request**, Prns **0.593623 mJ/request**. |

The throughput summary gives each of the ten scenarios on each of the three
hosts equal weight. It compares Prns-to-Prns with RNS-to-RNS for endpoints and
the two relay implementations behind the same driver for transport. It includes
both default-policy and policy-matched profiles, but excludes mixed endpoint
pairings. Ratios use unrounded medians of three samples and each scenario's
primary throughput metric. The middle half uses linearly interpolated 25th and
75th percentiles (`statistics.quantiles(ratios, method="inclusive")`). This is
a description of the published matrix, not an average expected application
speedup; exactly 15 of its 30 ratios fall within a strict 5×–25× interval.

Memory compares the largest process RSS peak across three samples in the same
role, not combined endpoint memory. Some single-packet responder rows use
slightly more memory than stock. Percentage reductions are
`100 × (1 − Prns / reference)`: 97.24% for the memory example and 98.10% for
the energy example, rounded to whole percentages in the headline copy. Energy
uses the medians of `net_millijoules_per_delivered` in the
[Linux request/response evidence](results/x86_64-unknown-linux-gnu/suites/987feb9e-471f-4e7b-aecc-321a60cd214c/request-response/).
It measures the combined processor package above idle. Per-role energy columns
are CPU-time attributions; they are not separate power measurements and should
not be substituted for the combined figure. Windows has no published energy data.

These peaks come from different workloads. All runs use loopback on the named
machines; physical network capacity, radio airtime, application work, and
whole-device power can dominate real deployments. The current suite pointers
are [macOS](results/aarch64-apple-darwin/current.json),
[Windows](results/x86_64-pc-windows-msvc/current.json), and
[Linux](results/x86_64-unknown-linux-gnu/current.json). Recalculate this summary
and the README, migration guide, and NomadNet copy whenever those pointers change.

## Check and smoke

```console
./tools/prns doctor benchmarks
cargo benchmark --smoke
```

The doctor checks Rust 1.90+, Python 3.11+, `uv`, and the platform C compiler
(on Windows that is MSVC's `cl`, from the Visual Studio Build Tools C++
workload; run the doctor as `.\tools\prns.cmd doctor benchmarks`).
It reports setup guidance and installs nothing. The smoke run exercises
participants, reference provisioning, calibration, measurement, and result
validation with reduced work.

## Full local run

```console
cargo benchmark
```

Cells run one at a time. The harness uses `uv` to provision its pinned Python
and stock interpreted RNS environment, records the source fingerprint and tool versions,
and retains a run ID. Local output is ignored by Git and is not publishable
evidence.

If a run is interrupted, resume only its missing samples:

```console
cargo benchmark --resume RUN_ID
```

Resume accepts only exact conformant samples from the same source SHA, source
fingerprint, and release profile. Any changed source invalidates the checkpoint.

## Publish qualified results

Maintainers publish from a clean exact commit:

```console
cargo benchmark --publish
```

Publication requires the complete matrix and updates the immutable suite before
changing its `current.json` pointer. It fails closed on incomplete conformance,
mixed source identity, insufficient harness headroom, or missing required
evidence. Read [Benchmark qualification](CONTRIBUTING.md) before publishing.

## Energy

```console
cargo benchmark --energy
```

- macOS explicitly authorizes `powermetrics` through `sudo`.
- Linux uses readable RAPL counters and fails if requested energy is missing.
- Windows does not support energy evidence.

Energy is optional evidence and never the performance sort key.

## Microscope profiling

Use the small component microscopes to answer a focused “where is the work?”
question before changing a hot path. The exact commands, sampling tools, and
artifact interpretation live in [Profiling](PROFILING.md). Do not substitute a
micro-benchmark improvement for end-to-end scenario conformance.

## Interpret results

Start with `RESULTS.md`, which is generated from the immutable current suites.
Each row belongs to a named scenario and implementation role. Compare:

- conformance and exact delivered work before speed;
- carried application payload rather than encoded wire rate;
- initiator and responder CPU/RSS in their recorded roles;
- latency only within the same scenario contract;
- host results only with their captured toolchain and machine provenance.

Default-policy and policy-matched rows intentionally exercise different interface
bitrate/MTU policy. Raw transport rows isolate relay work and exclude endpoint
crypto. Energy may cover the whole cell rather than one role. Durable details
and all pass rules remain canonical in
[Benchmark qualification](CONTRIBUTING.md).
