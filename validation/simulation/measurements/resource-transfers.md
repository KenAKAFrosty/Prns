# Mixed-runtime Resource transfer evidence

Host: macOS arm64. Local candidate based on `e6fe30134`, not hosted CI or
exact-SHA release evidence. Shipping code and firmware capacities are unchanged.

The [scenarios](../tests/embassy_ble/resources.rs) cover a Resource request and
echo, concurrent Resource replies, exact encoded response-envelope limits,
typed refusal at one byte below that envelope, reuse after refusal, and teardown.
They run against both ESP32/Apple and nRF52/BlueZ shared protocol endpoints.

A first experiment required all transfers to settle without time advancement.
The 1,200-byte upload reached its handler, but its response needed the Resource
retry timer. Letting the controlled clock advance completed the same bytes
without increasing the two-frame Embassy egress or four-entry GATT queues.
The committed queue policy is not changed or claimed lossless by these tests.

The test clock keeps existing timeless completions at 128 polls. Resource
operations explicitly allow 256 polls per tick, at most one millisecond per
idle advance, and a ten-second deadline. A diagnostic run observed the final
upload completing in 131 polls at one tick, motivating the separate workload
budget; this is not a throughput measurement or a shipping scheduler change.
Returned RTTs are compared with independently observed controlled elapsed time.
Four additional tests extend the previous six-test binary: two Resource
scenarios and two bounded-completion clock regressions.

## Verification

All listed checks passed, including all ten `embassy_ble` tests.

Host Cargo commands use `CARGO_INCREMENTAL=0`; commands run from repository root:

```console
cargo fmt --all --check
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy
cargo test --locked
cargo test --workspace --locked
for resource_run in {1..10}; do cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble resources || exit 1; done
```

The Resource scenarios are also repeated in ten separate test processes with
normal production Tokio entropy. Repetition checks stability, not deterministic
byte-for-byte replay or a performance benchmark. Existing ignored workspace
tests remain ignored. No shipping mutation surface changed; no mutation audit
was run for this test-only slice.

## Limits and follow-up

Payloads use a checked production uncompressed path; compression workers,
segmentation, persistence, arbitrary faults, and many-node Resource workloads
remain uncovered here. Host fixtures use explicitly bounded leaked allocations
for static Embassy APIs and growable engine storage. No native OS BLE API, HCI,
RF, firmware build, board memory contract, Miri/ISA, stock-Reticulum interop,
other host platform, or full PR-lane checks were run for this slice.

Resource response limits currently include the encoded envelope. A
[shared-core follow-up](../../../prns-core/plans/response-size-accounting.md)
records the inconsistent packet/Resource/completion-buffer accounting; these
tests do not assert that all transports expose a uniform application-byte limit.
