# Segmented responses at the completion-buffer boundary

Local macOS arm64 candidate based on `710bb216c`, 2026-09-26.

## Scope

This is additional deterministic simulator assurance, with no production changes.
It closes the 2 KiB segmented-completion boundary left by the previous cancellation
slice. Shipping buffers, allocation limits, wire formats and firmware code are
unchanged; the test-only file fixture grows by one byte.

Two endpoint pairings exercise real Tokio and Embassy runtimes over virtual BLE:
ESP32/CoreBluetooth and nRF52/BlueZ. These select compatibility behavior; they do
not run the native OS Bluetooth stacks, physical boards or RF.

Each pairing requests 2,047, 2,048, 2,049 and then 2,048 bytes again under a
2,048-byte budget, with both nodes taking the requester role. A test-only 512-byte
Resource window forces five segments. Metadata-bearing file bytes include an
envelope-shaped prefix, which must remain literal.

Raw journal calibration proves ordered, exact chunks and one terminal result.
The one-byte-over case publishes four provisional chunks before refusing the
fifth as `ResponseTooLarge`. It cannot publish the offending chunk or report a
successful partial response. Exact-capacity success preserves all 2,048 bytes.

Each size also runs through the real buffered request APIs. Tokio
uses an explicit request limit; Embassy derives it from its actual completion
capacity. Failures return only the typed error, not retained partial bytes. The
next exact-capacity request succeeds on the same links, and both bounded Embassy
completion slots can subsequently serve concurrent requests. Visible embedded
responder settlements distinguish success from `RejectedByPeer`; Tokio sender
reclamation is additionally exercised by subsequent transfers on its single slot.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`.

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble segmented_responses_honor_the_completion_capacity --quiet`: both new tests passed.
- `python3 validation/run.py run --suite virtual-device-simulation`: all passed,
  including 54 mixed-runtime BLE tests.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

This slice changes test fixtures and assertions only. Firmware builds, Miri/ISA,
physical hardware, interoperability and performance measurements were not rerun;
the previous slice records that production-change evidence. Arbitrary adversarial
segmentation, overlapping whole/split ownership and every compression mode are
not claimed by these boundary scenarios.
