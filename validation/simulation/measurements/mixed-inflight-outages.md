# Mixed-medium in-flight outage boundaries

macOS arm64, based on `e1fc2cbb7`, 2026-09-27.

## Scope and hypothesis

An outage matters only to the hops a packet still needs. A response already
queued at a frame/BLE bridge has finished its frame hop: cutting the frame
medium alone must not prevent delivery to its BLE requester. A request queued
at the same boundary still needs the frame path for its returning response.
Cutting both media prevents completion at either boundary.

Four tests cover request/response boundaries and frame-only/both-media cuts.
Each runs both CoreBluetooth/BlueZ profile assignments with cyclic ordering and
version-one scheduler seeds 0, 7 and u64::MAX. There are 32 combinations, each
with two interruption/recovery cycles. These are real Tokio production nodes
over virtual frame and GATT adapters, not native OS Bluetooth stacks.

## Exact boundary

The test polls one actor at a time until the frame trace shows a transmission
from the frame-only endpoint whose parsed header has the selected request or
response context and the exact logical link address. The same trace must show
that exact transmission ordinal queued at the bridge, at the unchanged clock
tick. The observed poll must belong to the sending node's exact actor ID.
No receiving-node poll occurs between this observation and the cut.
Trace truncation, early completion, unexpected idleness and poll-budget exhaustion
fail the boundary check rather than weakening it.

Before each cut, an uninterrupted control stops at that same boundary, then
checks the whole successful response. This establishes that the observation
corresponds to a viable exchange, not an already-broken route or link.

## Outcomes

| Queued frame | Cut | Required result |
| --- | --- | --- |
| Request toward BLE responder | Frames only | Exact 50 ms timeout: response cannot return |
| Response toward BLE requester | Frames only | Exact payload delivered without advancing time |
| Request toward BLE responder | Both media | Exact 50 ms timeout |
| Response toward BLE requester | Both media | Exact 50 ms timeout |

Frame-only cases retain the BLE connection and prove BLE-local traffic during
the outage. Both-media cases prove connection/member removal, then restore
frames first: frame-local traffic succeeds while fresh crossing and BLE-local
requests still time out exactly. Restoring BLE finally recovers all three
original logical links without restarting any node or re-establishing links.

Each recovery checks routes and ingress interfaces, new 256-byte echo values,
unchanged actor IDs, actor-count settlement, node elapsed time and coordinated
frame/BLE/runtime clocks. Final shutdown verifies complete detach, no pending
frame deliveries or BLE connections, and untruncated traces without reception
queue or discovery-observation overflow.

## Limits and production impact

No shipping source or production behavior changes. This adds evidence for
existing forwarding and settlement behavior. It observes the whole-frame ingress
queue; it does not inject a cut between GATT fragments or model radio airtime.
A frame already queued remains available to the receiver after a topology cut;
the test does not pretend the cut retroactively erases that accepted frame.
The shared fixture retains eight actor slots, two frame endpoints, two radios,
one peer per BLE supervisor and 20-byte GATT values. Seeds choose actor order,
not cryptographic entropy or complete byte-for-byte replay.

## Verification

The focused four-test matrix, root tests, workspace tests and all-target
simulation clippy with warnings denied passed on macOS arm64. The registered
simulation suite passed in 22.952 seconds; formatting/docs checks and
`git diff --check` also passed. Existing ignored
tests remain ignored. Host Cargo runs used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet mixed_outage_with --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

No hardware, firmware build, native Bluetooth, Miri/ISA or performance benchmark
result is claimed.
