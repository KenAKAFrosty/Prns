# Bounded BLE wire replay

## Scope and evidence

Two real Tokio nodes use the shipping BluetoothAuto supervisor over the virtual
BLE backend. Their fixed fixture identities, host runtime streams, shared
handle/interface streams, path-source policy, timeline, topology and actor order
are explicit. Discovery converges, each node announces, a Reticulum link opens,
and a 256-byte request/response crosses 20-byte GATT values. All node actors,
connections and radio registrations retire before returning the transcript.

Three fresh runs compare whole discovery traces, accepted control values,
accepted encoded GATT fragments and application responses without normalizing
away keys, signatures or ciphertext. A changed host seed changes the wire bytes
while retaining the response; an equal-length changed payload changes both.
The fixtures reject trace/capture overflow and dropped discovery observations.

The current BluetoothAuto runtime does not draw shared seam entropy in this
scenario; fixed BLE identities are supplied explicitly. The shared stream is
still installed before attachment. An unexpected path-source request or periodic
reseed fails the fixture rather than falling back to OS entropy. This is not a
claim of new BLE-specific random-source consumption or coverage of every source
lifecycle; those remain distinct from the preceding shared-consumer tests.

## Capture contract and cost

`BleWireCapture` is opt-in and bounded in whole characteristic values. It keeps
direction, channel and exact accepted bytes. Oldest values are evicted with an
explicit discard count, and snapshots are independent clones. Disabled labs
retain no wire payloads or capture buffer. Simulator connection/network state
adds an optional shared pointer and a branch per accepted value. Enabled capture
adds locking and payload allocation/copying; snapshot cloning has an additional
bounded cost. This round makes no performance or allocation-free claim.

Focused tests cover whole-value eviction, clone sharing, snapshot independence,
malformed-but-queued control values, capacity-blocked cancellation, oversize and
closed-link refusals, directionality, exact queued fragments, partial-send
cancellation, and intact whole-frame reception.

Acceptance is not delivery: cancelled sends may already have accepted fragments,
and queued data may later be lost on close. Capture order is the observation
order under the manual serial runner, not a cross-thread queue linearization
guarantee. Addresses do not identify connection incarnations. The current
fixture has no reconnects; add incarnation identity before claiming replay
through reused-address reconnects. This is not a stable serialized artifact.

The subsequent [incarnation follow-up](ble-connection-incarnations.md) adds
that identity and records both the narrower reconnect data-byte evidence and
the remaining internal arbitration input discovered by the stronger test.

CoreBluetooth/macOS and BlueZ/Linux are protocol endpoint profiles here, not
executions of native OS stacks. RF, controllers, firmware, other platforms,
worker scheduling, arbitrary time jumps and long-duration acceleration remain
outside this evidence.

## Production behavior

None changed. Capture and fixtures live entirely in the simulation package.
Production entropy defaults, wire formats and Bluetooth behavior are unchanged.

## Verification

Passed on macOS arm64; host Cargo invocations used `CARGO_INCREMENTAL=0`.

```console
cargo test -p prns-simulation --lib capture --quiet
cargo test -p prns-simulation --features controlled-time --test manual_fleet ble_replay --quiet
cargo clippy -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

Four focused capture tests and two BLE replay tests passed. The registered
simulation suite passed in 87.959 seconds, including 95 manual-fleet tests.
Its existing opt-in memory probe remained ignored. No firmware builds, hardware
smoke tests, native radio-stack tests or performance benchmarks were run for
this simulation-only change.
