# Benchmarks

This standalone crate answers one deliberately narrow release question: how does Prns compare with the compiled RNS 1.4.0 reference on the protocol's core data paths?

The public suite contains five scenarios:

- `single-packet-throughput` — native one-shot packet throughput.
- `link-message-throughput` — small messages over an established link.
- `request-response` — request/reply capacity and fractional-millisecond RTT.
- `resource-max-segment` — repeated maximum-efficient resource segments.
- `resource-64mib-stream` — sustained large-resource streaming.

Every scenario runs the same four directional pairings: Prns → Prns, Prns → reference, reference → Prns, and reference → reference. There are no third-party ports, capability exceptions, shared-instance/control-RPC lanes, or hand-selected comparison gaps.

## Release workflow

Build both participants as your normal user:

```sh
./build.sh
cargo run --release --bin describe_host
```

Inspect the complete 20-cell matrix or run a short non-publishing smoke:

```sh
target/release/benchmark_runner suite release --samples 3 --dry-run
target/release/benchmark_runner suite release --smoke
```

Run and render the three-sample release matrix:

```sh
./run-release-matrix.sh
cargo run --release --bin render_results
cargo run --release --bin render_results -- --check
```

On macOS, running the matrix through `sudo env "PATH=$PATH" …` may add processor-energy measurements. Energy is optional and never decides whether a cell passes. It is shown only when all three samples are above the fresh idle baseline; unavailable or noise-floor measurements remain blank.

Run one pairing directly with the same participant contract:

```sh
target/release/benchmark_runner run request-response \
  --initiator personal-rns --responder rns-1.4.0-compiled
```

Publishing requires release binaries and exactly three 30-second samples. Throughput and latency render as medians; role memory renders as maximum peak RSS. Each cell is staged outside `results/` and replaces its prior JSONL only after the complete sample set is conformant.

## Participants and reference proof

`implementations/` must contain exactly two descriptors: `personal-rns.json` and `rns-1.4.0-compiled.json`. A participant descriptor is only an executable command array; both participants are required to implement every role in every public scenario.

`reference/prepare-compiled-reference.sh` syncs the locked RNS 1.4.0, Cython, and setuptools environment and warms `reference/.object-cache/`. `compiled_reference.py` refuses to run unless the version is exactly 1.4.0, `RNS.compiled` is true, and a native RNS module is loaded. Python, Cython, compiler, RNS, native-module, and cache details are copied into result provenance.

## Results and measurement meaning

Every JSONL row carries schema version, run ID, sample index, scenario/version, host, metric, commit, toolchain, device/submitter IDs, provenance, and a direct initiator/responder subject.

The renderer orders detailed rows by throughput, never by memory or energy. Memory is reported separately for initiator and responder. Packet/request energy is per delivery; resource energy is normalized per application MiB so differently sized transfers remain intelligible. Processor energy is a package-level measurement; its initiator/responder split is an explicit CPU-time attribution, not a per-process power counter.

Request RTT is measured as wall-clock time from issuing a request until its settlement and is filed with fractional-millisecond precision on both participants. This prevents a fast sub-millisecond loopback result from being rounded into a false `0.00 ms`.

Do not hand-edit generated Markdown. Add or refresh raw rows, run the renderer, and commit both together.
