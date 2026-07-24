# Benchmarking Prns

The benchmark harness compares Prns and the locked reference implementation
under identical scenario-owned workloads. It records conformance, throughput,
latency, CPU, memory, and optional energy evidence. A smoke run checks the
machinery; only a complete qualified publication supports a release claim.

## Check and smoke

```console
./tools/prns doctor benchmarks
cargo benchmark --smoke
```

The doctor checks Rust 1.90+, Python 3.11+, `uv`, and the platform C compiler.
It reports setup guidance and installs nothing. The smoke run exercises
participants, reference provisioning, calibration, measurement, and result
validation with reduced work.

## Full local run

```console
cargo benchmark
```

Cells run one at a time. The harness uses `uv` to provision its pinned Python
and RNS/Cython environment, records the source fingerprint and tool versions,
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

Default-policy and “unleashed” rows intentionally exercise different interface
bitrate/MTU policy. Raw transport rows isolate relay work and exclude endpoint
crypto. Energy may cover the whole cell rather than one role. Durable details
and all pass rules remain canonical in
[Benchmark qualification](CONTRIBUTING.md).
