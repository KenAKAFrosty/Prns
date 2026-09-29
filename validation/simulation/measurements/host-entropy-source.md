# Tokio host entropy source boundary

Scope: source-selection groundwork for deterministic replay, not packet replay.

## Three slices

1. `TokioHost<S>` consumes an initialized shared-core `RuntimeEntropy<S>` with
   an explicit clock origin. Existing `TokioHost::new` and `start_at` remain
   OS-backed. The default type remains the OS-backed host; no box, lock or
   callback dispatch is added to host random fills.
2. Host tests compare whole output against the same core stream across a
   partially consumed handoff, a failed periodic reseed and later recovery.
   They assert source call counts and health, so a hidden OS fallback cannot
   masquerade as successful source control. A timeline reset cannot rewind
   the stream. These are adapter unit tests, not simulator discoveries.
3. Path discovery moves into its own owner module and calls the existing OS
   provider through a private fallible source seam. Tests inject a partially
   written failing read and prove no timing lookup, command ID allocation or
   command submission occurs. Successful reads preserve complete identifiers,
   timing, repeated-call consumption, settlement and stopped-node errors.

## Production impact and limits

No intended shipping protocol, randomness or timing change. Normal nodes still
seed host and handle streams from the OS and obtain path IDs through fallible
OS reads. Shared-core generator/reseed policy is unchanged. Scripted sources
live only in tests; the public host API requires a branded initialized stream.
Source quality still depends on the source implementation's contract.

The full `PrnsNode` still owns the default OS-backed host. The handle/interface
stream is likewise not externally source-selectable, and the path-request seam
is private. These must be connected in an explicit construction design before
claiming complete-node entropy control, restart replay or packet-byte replay.
No new full-node replay evidence is claimed in this round.

## Verification

Passed on macOS arm64. Host Cargo invocations used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib entropy_tests --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib path_discovery --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --quiet
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
python3 validation/run.py run --suite integration-capstones
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

Tokio: 281 tests passed, one timing probe ignored, one compile-fail doctest
passed. The registered simulator suite passed in 88.668 seconds and integration
capstones in 38.101 seconds. Integration ran with local socket access. No edited
owner is selected by the current mutation configuration. Firmware, hardware,
other operating systems and performance benchmarks were not run for this
Tokio-only boundary change.
