# Mixed Embassy/Tokio BLE replay

The Embassy checkpoint proved exact traffic for two Embassy nodes; earlier
mixed-runtime scenarios proved successful traffic and recovery but used ordinary
Tokio entropy and did not compare whole wire transcripts. This slice closes that
bounded mixed-runtime replay gap using the existing production construction APIs.

## Scenario and controlled inputs

One real Embassy node and one real Tokio node run on the coordinated timer/medium
driver. The scenario:

1. Discovers the pair and announces both destinations.
2. Establishes two protocol links and concurrently echoes 256 bytes in both directions.
3. Isolates the radios, settles teardown and verifies the retired connection
   accepts no additional wire values.
4. Restores reachability, discovers again, establishes distinct new protocol
   links and repeats both concurrent requests.
5. Drops the actors and verifies no active BLE connections remain.

The transcript includes every accepted control value and data fragment, discovery
events, both rounds of application responses and the reconnection boundary. No
capture eviction is allowed. Each phase uses exactly one BLE connection identity,
and the recovered phase must have a different identity from the original phase.

The fixture supplies Embassy's existing shared entropy source, a fixed logical
Tokio boot origin, separate Tokio host and handle entropy streams, inline crypto,
and explicit readiness arbitration. Unexpected path-entropy use or Tokio source
reseeding fails the scenario rather than falling back to uncontrolled inputs.
These are test sources, not a shipping deterministic-entropy mode.

The existing Tokio replay BLE selector is now shared test support, unchanged in
behavior and retaining its rotation/wake/cancellation tests. Both replay targets
use that implementation rather than maintaining copies. The common mixed-runtime
discovery and link-establishment helpers are also reused. Ordinary interop tests
retain their production-default Tokio construction.

## Evidence and boundaries

The matrix covers ESP32/BlueZ and nRF52/CoreBluetooth protocol profiles. For each,
all five initial BLE selector positions combine with both interface-driver
positions: twenty configurations total. Each configuration compares four fresh
complete runs. A changed payload must change responses and wire traffic; changing
either Embassy entropy or Tokio host entropy independently must change wire
traffic without changing application responses. This totals 140 bounded runs.
Different readiness orders need not produce identical traces; each must replay
its own complete result.

These profiles are protocol metadata, not execution of native BLE stacks or
firmware. This is cyclic outer-actor scheduling, two-node direct traffic, no
Resource transfer, no process restart, no source reseed and no path discovery.
It is not arbitrary scheduler determinism, large-fleet coverage or a serialized
replay format. Embassy static fixture allocations still remain until process exit;
the separate isolated heap probe owns memory evidence.

Production impact: none. No runtime, protocol, RNG or firmware behavior changed.
This adds cross-runtime replay assurance and consolidates test-only selection.

## Verification

Host: macOS arm64. Cargo commands use `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble replay::mixed --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

Hardware, native radio/controller behavior, other operating systems and embedded
ISA execution were not run for this test-only slice. Next parity milestones are
full Embassy node restart replay and larger Embassy fleet/memory coverage.

The registered simulation suite passed in 63.3 seconds. The focused mixed-replay
matrix also passed in three additional fresh test processes; these elapsed times
are verification observations, not comparative performance measurements.
