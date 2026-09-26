# Mixed-runtime BLE correctness evidence

Host: macOS arm64. This records local checks for the uncommitted slice based on
`5b52647d5`; it is not hosted CI or exact-SHA release evidence.

The [scenario](../tests/embassy_ble/interop.rs) exercises complete Tokio and
Embassy nodes over virtual GATT. Restoring only the prior Tokio command dispatch
makes `esp32_and_apple_nodes_exchange_encrypted_requests_and_recover` fail at the
expired-link request with `operation stalled before completion`. Restoring the
typed shared-core transport plan makes all six `embassy_ble` tests pass. This
is independent regression evidence, not a mutation-score justification.

## Passing checks

Host Cargo checks used `CARGO_INCREMENTAL=0` to limit retained build storage.
Commands run from the repository root:

```console
cargo fmt --all --check
cargo fmt --manifest-path prns-runtime/impls/tokio/Cargo.toml --check
cargo test --locked -p prns-core routing::links::request::tests
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble
cargo clippy --locked -p prns-core -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-targets -- -D warnings
cargo check --locked -p prns-core --no-default-features
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml
cargo test --locked
cargo test --workspace --locked
python3 validation/run.py run --suite bluetooth-auto-embassy
python3 validation/run.py run --suite virtual-device-simulation
CARGO_TARGET_DIR=target/repo-guards cargo run --manifest-path ../contributing/repo-guards/Cargo.toml
```

The focused request owner has 23 passing tests; the separate Tokio workspace
has 252 passing unit tests and one compile-fail doc test. The registered Embassy
BLE suite has 55 passing tests. Existing explicitly ignored tests remain ignored.

These checks do not exercise native Apple/BlueZ APIs, HCI controllers, RF,
firmware builds, board resource contracts, or physical devices. Endpoint labels
select shared protocol behavior. The fixture uses host storage, bounded leaked
static allocations, and production Tokio entropy; it is not an embedded-memory,
large-fleet, throughput, or byte-for-byte replay measurement.

Resource-backed request failures after transport planning remain a separate
[settlement and receipt-cleanup follow-up](../../../prns-runtime/impls/tokio/README.md#request-admission).

## Focused mutation audit

```console
CARGO_INCREMENTAL=0 cargo mutants --no-config -p prns-core -f prns-core/src/routing/links/request.rs -F 'plan_request_transport|request_fits_packet' --in-place --output validation-artifacts/request-transport-mutation --timeout 120 -- --locked --lib routing::links::request::tests
python3 validation/run.py mutation-shard-check --results validation-artifacts/request-transport-mutation/mutants.out/outcomes.json
```

cargo-mutants 27.1.0: baseline passed, seven mutants caught, one unviable, no
survivors or timeouts. Manual inspection confirmed that the unviable mutant
substitutes `Ok(Default::default())`, but `RequestTransport` deliberately has no
default. No production rewrite or triage waiver was made for that mutation.
Explicit filters restrict this local audit to the changed decision and its
compatibility predicate; this is not the full scheduled mutation audit.
