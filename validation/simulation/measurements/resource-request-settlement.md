# Resource request settlement evidence

Host: macOS arm64. Local candidate based on `974a9b1d0`, not hosted CI or
exact-SHA release evidence. Host Cargo commands used `CARGO_INCREMENTAL=0`.

The [mixed-runtime regression](../tests/embassy_ble/interop.rs) sends an oversized
Resource request from Tokio to Embassy over virtual GATT. Before the settlement
fix, it stalled with `operation stalled before completion`. With the fix, both
endpoint pairings return `RequestTransferFailed(RejectedByPeer)` without advancing
time, then exchange ordinary requests successfully on the same link.

Twelve focused core tests cover admission and build failure, receipt ownership,
peer rejection, both timeout orderings, upload proof, link closure, eviction,
response-before-proof, and segmented-request refusal. Whole request envelopes
were already the receive contract; unsupported segmented uploads now fail before
advertisement rather than creating several owners for one request result.
Accepting a Resource response retires its obsolete upload so a lost-proof
watchdog cannot terminate the response. Segmented responses remain supported.

## Passing checks

Commands from the repository root:

```console
cargo fmt --all --check
cargo fmt --manifest-path prns-host/impls/native/Cargo.toml --check
cargo fmt --manifest-path prns-napi/Cargo.toml --check
cargo fmt --manifest-path prns-wasm/Cargo.toml --check
cargo test --locked -p prns-core routing::links::resources::settlement
cargo clippy --locked -p prns-core -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo check --locked -p prns-core --no-default-features
cargo check --locked --manifest-path prns-host/impls/native/Cargo.toml --all-targets
cargo check --locked --manifest-path prns-napi/Cargo.toml --all-targets
cargo check --locked --manifest-path prns-wasm/Cargo.toml --target wasm32-unknown-unknown
cargo test --locked
cargo test --workspace --locked
python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy
python3 validation/run.py run --suite interop-large-request --suite interop-resource-rejection
CARGO_TARGET_DIR=target/repo-guards cargo run --manifest-path ../contributing/repo-guards/Cargo.toml
```

The stock-Reticulum interoperability suites required local socket permission;
the sandboxed attempt failed with `Operation not permitted`, and the authorized
rerun passed. These are unchanged upstream compatibility checks, not a claim of
new wire behavior. Existing explicitly ignored workspace tests remain ignored.

## Focused mutation audit

```console
CARGO_INCREMENTAL=0 cargo mutants --no-config -p prns-core -f prns-core/src/routing/links/resources/settlement/mod.rs -f prns-core/src/routing/delivery/receipts/core.rs -f prns-core/src/routing/links/request.rs -F 'claim_resource_response|settle_advertised_resource|resource_failure_settlement|take_request_for_command|book_request_resource_receipt' --in-place --output validation-artifacts/resource-request-mutation --timeout 120 -- --locked --lib routing::links::resources::settlement
python3 validation/run.py mutation-shard-check --results validation-artifacts/resource-request-mutation/mutants.out/outcomes.json
```

cargo-mutants 27.1.0: baseline passed, 13 mutants caught, three unviable, no
survivors or timeouts. Inspection confirmed all unviable mutations require
nonexistent `Default` implementations (`WakeSchedules`, `ProvenRequestReceipt`,
`Settlement`). No waiver or mutation-driven production rewrite was needed.
Explicit filters cover the changed ownership functions, not the full surface.

## Limits

Virtual endpoint labels select shared protocol behavior, not native Apple/BlueZ
APIs, HCI controllers, or RF. No firmware builds, board resource contracts,
Miri/ISA checks, hardware tests, other host platforms, full PR lane, or scheduled
mutation/fuzz campaign were run for this slice. No persistent buffer was added;
that is not a measurement of firmware RAM, stack, throughput, or fleet scale.
