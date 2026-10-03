# Overlapping bridge outages

macOS arm64, based on `859fa4253`, 2026-09-27.

## Hypothesis and scope

A frame/BLE bridge needs both media for crossing traffic. Restoring either
medium alone must restore that side's local link without falsely restoring
end-to-end delivery. Short medium outages must not require node restart or new
logical links when the production engines retain their state.

Two manual-fleet tests cover frame-first and BLE-first recovery. Each runs both
CoreBluetooth/BlueZ profile assignments and four scheduling policies: cyclic
admission order and version-one seeded order with seeds 0, 7 and u64::MAX.
There are 16 combinations, each with two outage/recovery cycles. These profiles
run over virtual GATT; no native OS Bluetooth stack or physical radio is used.

## Assertions

Three real Tokio production nodes retain their actors and clock origins:
a frame-only endpoint, a dual-interface transport and a BLE-only endpoint.
Four logical links are established before disruption: one crossing in each
direction, one frame-local link and one BLE-local link.

- All four links first exchange concurrent, lane-tagged 256-byte echo values.
  Exact task-to-response maps prevent completion-order assumptions or swapped
  responses from passing the test. Successful request rounds do not move time.
- Both media are isolated before actors are polled. BLE connection and member
  inventories become empty without advancing time or replacing node actors.
- All four paths time out at exactly 50 milliseconds, with no early settlement.
- The first recovered medium restores only its local path. Requests on both
  crossing links and the still-isolated local path remain pending and time out
  at exactly 50 milliseconds while the restored local path exchanges traffic.
- BLE-first recovery waits for both real supervisor member inventories, not
  merely a backend connection. It does not require announcements to cross the
  still-isolated frame medium.
- After the second medium recovers, route inspection confirms both crossing
  routes and their ingress interfaces. All four original links exchange exact
  concurrent responses again. The second cycle repeats with those same links.
- Node actor IDs, per-node elapsed time, both medium clocks and runtime time
  remain consistent. Final shutdown checks complete detach and bounded traces
  without dropped or truncated evidence.

The lab retains its existing bounds: three node actors, eight total actors,
two frame endpoints, two radios, one peer per supervisor and 20-byte GATT data
values. Four simultaneous requests use seven actor slots. This is overlapping
outage coverage, not an in-flight packet cut or a fairness/throughput benchmark.

## Production impact and limits

No shipping source or production behavior changes. The new evidence exercises
existing engines, routing, BLE supervisors, fragmentation and request settlement.
The test fixture now accepts an explicit scheduling policy; existing recovery
tests retain their cyclic policy. Seeds control actor order, not crypto entropy,
and this is not complete byte-for-byte replay or exhaustive interleaving coverage.

## Verification

Passed the focused matrix, all-target simulation clippy with warnings denied,
root tests, workspace tests and the registered simulation suite (25.669 seconds).
Existing ignored tests remain ignored. Host Cargo runs used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet overlapping --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

No hardware, native Bluetooth, firmware build, Miri/ISA or benchmark result is
claimed by this slice.
