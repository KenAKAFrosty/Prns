# Caller cancellation during fragmented request transmission

macOS arm64, based on `e14d39e65`, 2026-09-27.

## Question and result

Does dropping the caller during a partially transmitted request retract the
request or disrupt subsequent traffic? In these production Tokio-node scenarios,
it does neither: the original request finishes crossing BLE, the peer generates
a complete response, and replacement callers receive their own exact values.
This documents existing behavior, not a new cancellation policy.

## Matrix

Four new tests combine queued/consumed partial-request boundaries with
replacement admission before/after drain. Each covers both request directions,
both CoreBluetooth/BlueZ profile assignments, and cyclic ordering plus scheduler
version-one seeds 0, 7 and u64::MAX. There are 64 new configurations, each with
two cycles, alongside the previous 64 response-cancellation configurations.

The shared observer matches the exact `Request` header and logical link, then
requires two fragments queued, zero or two values consumed, no completed send
or reassembled frame, and the expected last-polled sender/receiver actor. The
clock does not advance. Cancelling the caller changes neither the BLE data
snapshot nor discovery trace, and the original connection stays up.

Before-drain cases admit two differently marked replacement requests before
any node is polled again. Their complete task-to-value map must contain only
their own responses. After-drain cases first settle the abandoned exchange and
require:

- No completion for any registered caller.
- Exactly one further completed send and reassembled frame in the interrupted
  request direction, without another send starting in that direction.
- A newly started response in the reverse direction whose parsed header has
  `Response` context and the same logical link address.
- Exactly one completed send and reassembled frame relative to that response's
  own baseline, with no time advancement.

The response-generation assertion also strengthens the existing after-drain
response-cancellation tests. It is not satisfied by a protocol proof, stale
header observation or fragments without a complete frame.

Both replacement timings retain the original checks: two concurrent exact
responses, frame-local traffic, silent passage through the old request's
50-millisecond deadline, another two-request round on the same link, unchanged
node actors, route/ingress checks, coordinated clocks and complete teardown.
All old/new payload markers are distinct across rounds and cycles.

## Production impact and limits

Test-only change, reusing existing simulator observations and the shared
cancellation fixture. No shipping behavior, allocation or cancellation policy
changes. The tests demonstrate why local cancellation cannot be used as proof
that a peer did no work; they do not establish exactly-once side effects or
immediate removal of private runtime completion entries.

These are virtual GATT profiles, not native Bluetooth stacks or physical radios.
No raw application-journal assertions, permanently unresponsive-peer retention
bound, firmware build, Miri/ISA or performance result is claimed.

## Verification

Passed all eight cancellation tests, simulation all-target clippy with warnings
denied, root/workspace tests and the registered simulation suite (33.912 seconds).
Repository formatting/docs checks and `git diff --check` also passed.
Host Cargo runs used `CARGO_INCREMENTAL=0`; existing ignored tests remain ignored.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet cancelled_ --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```
