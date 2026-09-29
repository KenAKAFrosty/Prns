# Tokio continuation release after Embassy request timeout

Local macOS arm64 candidate based on `0fc83ae8a`, 2026-09-26.

## Scope

Test-only mixed-runtime assurance. No production behavior, retained state,
capacity or wire format changes.

Reversing the previous blocked-send scenario found a distinct runtime path:
the Tokio responder's send remains held when Embassy's between-segment request
deadline expires. The existing Embassy-responder test still requires automatic
send cancellation, stale-link closure and fresh-link recovery. The new reverse
test explicitly observes the still-held Tokio send after expiry, then releases it;
it does not assume cancellation symmetry.

For each ESP32/Apple and nRF52/BlueZ profile pair, the test requires exactly one
verified first segment and one Timeout settlement for Embassy's raw request. It
also consumes the exact command settlement from the embedded diagnostic journal.
The Tokio gate must still hold the second continuation advertisement before its
manual release. After release and settling runnable work, the gate must be idle
and no new response event may appear.

Both bounded Embassy request slots are then reused concurrently for exact echo
responses on the original link. Existing reassembly checks exercise size refusals,
raw segmented delivery and buffered delivery in both directions, without creating
fresh links. The surrounding fixture forbids unexpected link closures, unsolicited
embedded Resource delivery, leftover response events or gates, and checks radio
cleanup at teardown. Later successful traffic also drives the simulated medium
after the released send, so the assertion is not limited to its dispatch instant.

This is a genuine mixed-runtime late-advertisement case using real encrypted
traffic through simulated BLE. It is not an independent conflicting sender,
duplicate-frame injection, worker-result replay, physical radio test, or a claim
that the two runtime cancellation policies should be identical.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`. Passed:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble blocked_send --quiet` (both runtime directions).
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- Registered `virtual-device-simulation`, now including 58 mixed-runtime BLE tests.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

No production mutation or firmware, ISA/Miri, physical hardware, stock
interoperability or benchmark rerun was performed for this test-only slice.
