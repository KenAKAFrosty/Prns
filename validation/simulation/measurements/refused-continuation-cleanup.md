# Refused continuation cleanup

Local macOS arm64 candidate based on `7c9b23dc7`, 2026-09-26.

## Motivation and reproduction

The segmented-response audit following the simulator's expiry/culling discoveries
still identified pre-admission refusal as an untested cleanup boundary. Four new
shared-core tests complete a first encrypted response segment, then refuse its
continuation because of response size, impossible transfer size, a full pending
offer queue, or expiration of the queue wait. Before the fix, all four returned
the correct terminal error but retained the failed request's assembly.

These are deterministic core-engine reproductions using real encrypted frames
and bounded storage, not newly discovered end-to-end simulator failures. Queue
pressure uses actual admitted transfers and offers; oversize/capacity cases
rewrite and re-seal a continuation advertisement at the authenticated peer
boundary. They do not claim an ordinary production sender emits those shapes.

## Production change

Terminal refusal now reuses the same link/request-scoped assembly cleanup as
receipt expiry and culling. Its internal input is a typed request ID; expiry and
culling derive that ID from the receipt's existing packet hash. No stored fields,
buffer capacities, wire formats or advertised response-size policies change.
Unsolicited, request-body and differently correlated response assemblies remain
untouched when another request fails.

The queued-offer watchdog also checks chain identity, next index, segment count
and correlation before evaluating terminal refusal. A removed, replaced, advanced
or otherwise changed chain makes its old queued continuation stale; that offer
is dropped without sending a receiver cancellation, failing the live receipt or
altering the replacement assembly. Promotion already checked this ownership;
the deadline/refusal path now does too.

The admission tests are organized as a private test-only module with a separate
refusal leaf. Their `std` allocations do not enter the no-std production build.
Six new tests cover the four failures, five stale-queue states and three unrelated
assembly correlations. The existing successful-continuation/promotion controls
remain in the same nine-test group.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; canonical firmware builds do not
override their environment. Passed:

- `cargo test --locked -p prns-core split_admission_tests --quiet`: nine tests.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`:
  all six suites passed, including all 52 mixed-runtime BLE tests.
- `bash validation/hygiene/fmt-docs.sh` (including both registry checks), and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`: 142,980 static
  bytes plus four bytes padding, 69,632 bytes runtime reservation and 376 bytes
  RAM headroom, unchanged. Existing stack evidence leaves 10,352 bytes, with its
  unresolved-call/frame gaps. Flash headroom is 145,420 bytes, 88 fewer than the
  preceding production slice; this is not a speed measurement. The first report
  attempt detected a source-state change while new files were being marked for
  review and refused evidence; the rerun completed with stable source state.

Focused mutation audit:

```console
CARGO_INCREMENTAL=0 cargo mutants --no-config --in-place -p prns-core \
  --file prns-core/src/engine/settlement.rs \
  --file prns-core/src/routing/links/resources/receive/watchdog.rs \
  --re 'retire_response_assembly|watchdog.rs:101:|watchdog.rs:93:.*> with ==' \
  --timeout 60 --build-timeout 180 \
  --output validation-artifacts/mutation/refused-continuation \
  -- --lib split_admission_tests
```

Baseline passed; all four mutants were caught, with no survivors or timeouts.
Removing cleanup or reversing its correlation comparison fails the refusal tests.
Disabling the queued-continuation check fails the stale-queue deadline test;
reversing its fit comparison also fails the focused group. These are selected
diagnostic mutations, not exhaustive coverage of the admission/watchdog surface.

`./tools/prns doctor embedded-assurance` reports about 1 GiB free versus the
24 GiB matrix requirement and missing Renode. Generated-contract checking still
refuses the pre-existing stale `tools/release/flasher_memory_contracts.py`;
its inventory inputs and output are untouched. The full board matrix, platform
pilots and physical hardware were not run.

The remaining segmented work includes cumulative delivered-value accounting,
exact completion-buffer limits, whole/split overlap arbitration, and moving
adversarial peer cases into the mixed-runtime simulator. Passing existing
simulation or Miri/ISA suites is regression evidence, not direct execution of
these new refusal injections on radios or target firmware.
