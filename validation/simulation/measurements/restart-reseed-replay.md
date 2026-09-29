# Host packet replay across restart and reseeding

This extends the bounded frame-only evidence in
[node-host replay](node-host-replay.md), not the entire runtime's replay claim.

## Three connected slices

1. Separate the test fixture, scenario assertions and scripted source into
   owned modules. Source reads now carry node identity, boot generation,
   attempt number and a typed success/failure outcome. Scripts and logs are
   bounded; unexpected reseeds or exhausted scripts fail the test rather than
   silently providing extra bytes or falling back to the OS.
2. Replay an echo, advance to seven milliseconds, cancel the receiving node,
   construct its next incarnation with distinct source material, then establish
   a new link and echo again. The interface identity stays stable while task
   and medium endpoint identities change. The new boot uses the scenario's
   current timeline. Three runs compare complete traces, responses and source
   reads, including final shutdown. Changing only the restarted source leaves
   the first exchange byte-identical and changes subsequent packets.
3. Exercise successful and failed periodic reseeding during real node packet
   production. Before handoff, consume exactly one 64-KiB window from the core
   stream and assert that only its initial seed has been read. The node's first
   nonempty draw then triggers the actual core reseed. Each outcome repeats
   across three runs; changing fresh material or injecting failure changes
   packets while preserving the successful application response.

## Evidence boundaries

The reseed test deliberately primes a partly consumed stream. It does not claim
that this short network workload itself consumes 64 KiB of randomness, nor
does it reimplement the core generator or reseed algorithm. Source records prove
one initial seed and one reseed attempt per node in these scenarios. Failed
reseeding keeps the already-initialized stream usable under existing core policy.
Repeated failure and later recovery remain adapter/core unit-test evidence,
not a new long-duration simulator claim.

Restart is volatile actor cancellation and fresh node construction with stable
fixture identity. It is not physical power loss or durable-storage restoration.
Old/new endpoint separation, exact responses, actor teardown, empty delivery
queues, complete trace retention and balanced attachment/detachment are checked.

Normal node construction, production APIs and all shipping behavior are
unchanged in this round. Handle/interface and path-ID entropy still need source
selection before replay can expand to their consumers. BLE, worker completion
order, automatic deadline discovery and large-fleet performance remain separate
milestones. The next forward work is those input owners, not more variations of
this echo test. Transcripts remain private test values, not an exported format.

## Verification

Passed on macOS arm64; host Cargo commands used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet entropy_replay --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --test manual_fleet -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

All six focused tests passed. The registered simulator suite passed in 76.384
seconds (validation wall time, not a benchmark). No mutation-selected shipping
owner changed. Separate integration capstones, firmware builds, physical
hardware, other operating systems and performance benchmarks were not rerun
for this test-only slice.
