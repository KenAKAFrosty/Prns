# Benchmark qualification

This suite answers one release question: how does Prns compare with the compiled RNS 1.4.0 reference on ten core protocol profiles? Seven endpoint scenarios run all four Prns/reference directions; three transport scenarios run each implementation as the relay behind the same fixed wire driver, for a fixed 34-cell matrix.

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

This executes all 34 cells in three counterbalanced rounds. Cells run one at a time to isolate CPU, memory, and optional energy, while each workload retains its protocol-owned asynchronous window. A failed run prints the retained run ID; continue only its missing samples with:

```sh
cargo benchmark --resume RUN_ID
```

The suite checkpoint is installed before the first cell and refreshed atomically after every cell. Resume keeps only exact, conformant samples from the same source SHA, tracked/untracked source fingerprint, and release profile. An unchanged dirty local worktree can resume safely; any source edit invalidates it. A child that exits before `MEASURE_DONE` may receive up to three explicit startup attempts; every attempt and failure is printed, logged, and counted in suite evidence. Once measurement starts, a sample is never retried.

Maintainers publish a complete suite from a clean exact commit with:

```sh
cargo benchmark --publish
```

Publication copies the immutable suite under `benchmarks/results/HOST/suites/RUN_ID/`, then updates `current.json` last. The renderer follows only that pointer, so partial or mixed evidence cannot become current.

## Prerequisites and platform behavior

The frontend requires Rust/Cargo, `uv`, and a native C compiler. `uv` provisions pinned Python 3.13 and the locked RNS/Cython environment; do not create a Python environment by hand.

Release participants are built for the machine actually being qualified (`-C target-cpu=native`; every AArch64 host also enables RustCrypto's runtime-detected ARMv8 AES backend). Host builds default to the thresholded parallel resource-hash and persistence paths, matching prnsd and Hopspot production hosts. The exact flags and tool versions are captured in suite evidence, so a fast local build cannot masquerade as a portable binary measurement.

- macOS: ordinary runs explain that energy is absent. `cargo benchmark --energy` authorizes `powermetrics` through `sudo`; Cargo, Python, caches, participants, and outputs remain owned by the normal user.
- Linux: readable RAPL counters are used automatically. `--energy` fails rather than silently omitting required energy.
- Windows: energy is unsupported. Throughput, RTT, conformance, initiator/responder working set, and CPU attribution still run.

Energy is optional evidence, never the sort key and never a conformance shortcut.

## Workload ownership and pass rules

The ten `scenarios/*/manifest.json` files own scenario IDs, versions, order, topology, workload values, seeds, timeouts, typed conformance-rule selection, and their human explanation. Rust and Python derive identical deterministic size and payload vectors from them; `scenarios/workload-vectors.json` is a checked golden contract verified before measurement. The public catalog is:

- `single-packet-throughput`
- `link-message-throughput`
- `request-response`
- `resource-max-segment`
- `resource-max-segment-unleashed`
- `resource-64mib-stream`
- `resource-64mib-stream-unleashed`
- `raw-transport-throughput`
- `transport-resource-throughput`
- `transport-resource-throughput-unleashed`

Manifests may attach a typed `cell_notes` entry to an exact initiator/responder or relay subject. The renderer marks that row and repeats the interpretation beneath every host table where the subject appears. Use these notes for durable protocol-role context and corroborated cross-host findings; immutable result rows retain machine provenance and are not rewritten to carry later analysis.

Request/response keeps exactly four operations in flight over four pre-established links. Its manifest fixes the link MTU at the standard 500 bytes on both implementations: deterministic 32–128 byte requests stay below the link MDU, while 1–4 KiB responses unambiguously use the documented asynchronous resource-response path. Each implementation keeps one protocol-owned response Resource lane per link; Prns holds that lane through proof settlement before transmitting the next response, while all four links remain independently concurrent. This is the realistic public API shape and avoids both a same-link Resource overlap and the stock reference's sub-MDU loopback receipt race without sleeps, state polling, latency shims, or reference patches. Fractional RTT runs from request issue through the complete response Resource settlement. Every link is explicitly armed through the public request API before the measurement barrier; startup attempts are printed and recorded, and are not measurement retries.

Link-message rows require exact initiator sends, responder deliveries, and application-byte totals. Stock RNS can leave a small tail of packet receipts unproved after all bytes have arrived; those are exposed separately as `receipt_unproved` and never mislabeled as wire loss.

Resource rows are one logical stream: the protocol-owned part/window machinery remains asynchronous inside each resource, and the receiver sends an ordered application acknowledgement before the next logical resource starts. This prevents a transport proof from being mistaken for application delivery and makes sent, settled, received, and byte totals independently exact. The 64 MiB workload repeats the shared deterministic maximum-size block on both participants, so the wire bytes are identical while Prns can exercise its bounded-memory streaming API. Each resource workload has a default-policy row and a controlled `-unleashed` counterpart that explicitly configures both real endpoint TCP interfaces for 1 Gbps. Encryption, resource hashing, proof settlement, assembly, and application acknowledgement remain unchanged; only the interface bitrate policy and resulting MTU tier differ.

Raw transport is the open, HDLC-framed localhost TCP backbone ceiling. A fixed driver signs one announce on each side before measurement, learns the relay's real transport identity from its rebroadcasts, and proves one warm forward each way. The timed `raw-transport-throughput` path then holds up to 256 unique opaque SINGLE/DATA frames outstanding per direction with deterministic 60–420-byte payloads. Endpoint encryption, decryption, signing, and proof verification are outside that path; real relay SHA-256 deduplication, route and reverse-route bookkeeping, header rewriting, copies, buffering, framing, and TCP I/O remain. Returned opaque proof frames release driver credit and keep reverse-route state bounded.

The tiny raw SINGLE scenario is default-policy-only: changing the MTU tier cannot change its 60–420-byte frames. Prns therefore reports its normal 500 Mbps / 131,072-byte TCP policy and compiled RNS 1.4.0 its normal 10 Mbps / 8,192-byte policy, with no artificial 1 Gbps twin.

The transported-resource pair measures the large-frame switchboard separately. Before timing, the driver establishes one genuine transported link through the relay and validates its signed link proof. The timed path holds up to 16 full-size opaque LINK/DATA resource parts in flight per direction; receive-side driver accounting releases credits without injecting per-frame proof traffic. `transport-resource-throughput` preserves each relay's normal TCP policy, while `transport-resource-throughput-unleashed` explicitly configures both relay interfaces for 1 Gbps and requires the 524,288-byte tier. This changes the effective resource-part size, where MTU policy materially matters, while keeping endpoint crypto and assembly outside the relay measurement.

Before every relay sample, the driver runs a two-second full-duplex encode/transfer calibration using that scenario's actual frame shape. Publication requires calibration throughput at least 1.25× the measured carried-payload rate, so a feeder-limited run fails as `harness_headroom`; smoke runs exercise the calibration without enforcing its performance threshold. Conformance additionally requires nonzero traffic both ways, exact directional frame and payload-byte totals, and zero missing, duplicate, corrupt, reordered, unexpected, outstanding, or timed-out frames. Relay CPU and RSS belong only to the relay process. Any package-energy rows are explicitly whole-cell measurements and must never be described as relay-only energy.

A release suite is accepted only with 34/34 cells and 102 samples `{0,1,2}` across the matrix, one full 40-character source SHA, one scenario-version set, compiled RNS 1.4.0 proof, and clean scenario-owned accounting. Measurement samples are never silently retried.

## Evidence meaning

Participant startup, imports, identity creation, link establishment, and link arming occur before the measurement barrier. The responder remains silent until both processes report `READY`; the runner then records the startup attempt and releases announcements. This avoids sending into a reference TCP interface while it is still constructing. Both roles must subsequently report `MEASURE_READY`; only then does the runner start CPU/energy collection and release the initiator. After the deadline, outstanding protocol and application-acknowledgement work drains and the initiator emits `MEASURE_DONE`; package energy and role CPU stop there, before link teardown or reporting grace sleeps. Peak role memory remains full-process peak RSS/working set. Request RTT is wall time from issue through complete response settlement and retains fractional milliseconds.

Each suite records command lines, start/finish times, exit states, startup and measurement attempt counts, stdout/stderr logs, host/toolchain/reference proof, the exact source fingerprint, schedule order, and result-file hashes. Generated Markdown is read-only. Raw local runs are ignored; only `cargo benchmark --publish` promotes complete exact-SHA evidence and regenerates the tracked views.
