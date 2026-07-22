# Benchmark qualification

This suite answers one release question: how does Prns compare with the compiled RNS 1.4.0 reference on five core protocol paths? Every scenario runs all four Prns/reference directions, for a fixed 20-cell matrix.

## Operator interface

From the repository root:

```sh
cargo benchmark --smoke
```

That is the quickest confidence check. It provisions the locked compiled reference, builds release participants, exercises every direction, checks exact scenario accounting, and leaves an ignored diagnostic run beneath `benchmarks/.benchmark-runs/`.

For publishable local evidence, run:

```sh
cargo benchmark
```

This executes all 20 cells in three counterbalanced rounds. Cells run one at a time to isolate CPU, memory, and optional energy, while each workload retains its protocol-owned asynchronous window. A failed run prints the retained run ID; continue only its missing samples with:

```sh
cargo benchmark --resume RUN_ID
```

The suite checkpoint is installed before the first cell and refreshed atomically after every cell. Resume keeps only exact, conformant samples from the same source SHA, tracked/untracked source fingerprint, and release profile. An unchanged dirty local worktree can resume safely; any source edit invalidates it. Measurement failures are never retried invisibly.

Maintainers publish a complete suite from a clean exact commit with:

```sh
cargo benchmark --publish
```

Publication copies the immutable suite under `benchmarks/results/HOST/suites/RUN_ID/`, then updates `current.json` last. The renderer follows only that pointer, so partial or mixed evidence cannot become current.

## Prerequisites and platform behavior

The frontend requires Rust/Cargo, `uv`, and a native C compiler. `uv` provisions pinned Python 3.13 and the locked RNS/Cython environment; do not create a Python environment by hand.

Release participants are built for the machine actually being qualified (`-C target-cpu=native`; Apple Silicon also enables RustCrypto's ARMv8 AES backend). The exact flags and tool versions are captured in suite evidence, so a fast local build cannot masquerade as a portable binary measurement.

- macOS: ordinary runs explain that energy is absent. `cargo benchmark --energy` authorizes `powermetrics` through `sudo`; Cargo, Python, caches, participants, and outputs remain owned by the normal user.
- Linux: readable RAPL counters are used automatically. `--energy` fails rather than silently omitting required energy.
- Windows: energy is unsupported. Throughput, RTT, conformance, initiator/responder working set, and CPU attribution still run.

Energy is optional evidence, never the sort key and never a conformance shortcut.

## Workload ownership and pass rules

The five `scenarios/*/manifest.json` files own scenario IDs, versions, order, workload values, seeds, timeouts, typed conformance-rule selection, and its human explanation. Rust and Python derive identical deterministic size and payload vectors from them; `scenarios/workload-vectors.json` is a checked golden contract verified before measurement. The public catalog is:

- `single-packet-throughput`
- `link-message-throughput`
- `request-response`
- `resource-max-segment`
- `resource-64mib-stream`

Request/response keeps exactly four operations in flight over four pre-established links. Its manifest fixes the link MTU at the standard 500 bytes on both implementations: deterministic 32–128 byte requests stay below the link MDU, while 1–4 KiB responses unambiguously use the documented asynchronous resource-response path. This is the realistic public API shape and avoids the stock reference's sub-MDU loopback receipt race without sleeps, state polling, latency shims, or reference patches. Fractional RTT runs from request issue through the complete response Resource settlement. Every link is explicitly armed through the public request API before the measurement barrier; startup attempts are printed and recorded, and are not measurement retries.

Link-message rows require exact initiator sends, responder deliveries, and application-byte totals. Stock RNS can leave a small tail of packet receipts unproved after all bytes have arrived; those are exposed separately as `receipt_unproved` and never mislabeled as wire loss.

A release suite is accepted only with 20/20 cells, samples `{0,1,2}`, one full 40-character source SHA, one scenario-version set, compiled RNS 1.4.0 proof, and clean scenario-owned accounting. Measurement samples are never silently retried.

## Evidence meaning

Participant startup, imports, identity creation, link establishment, and link arming occur before the measurement barrier. After the deadline, outstanding protocol work drains and the initiator emits `MEASURE_DONE`; package energy and role CPU stop there, before link teardown or reporting grace sleeps. Peak role memory remains full-process peak RSS/working set. Request RTT is wall time from issue through complete response settlement and retains fractional milliseconds.

Each suite records command lines, start/finish times, exit states, startup and measurement attempt counts, stdout/stderr logs, host/toolchain/reference proof, the exact source fingerprint, schedule order, and result-file hashes. Generated Markdown is read-only. Raw local runs are ignored; only `cargo benchmark --publish` promotes complete exact-SHA evidence and regenerates the tracked views.
