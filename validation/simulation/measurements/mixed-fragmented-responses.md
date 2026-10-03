# Partial-GATT response interruption

macOS arm64, based on `d1ee7f421`, 2026-09-27.

## Why the boundary needs a header

The BLE data characteristic carries protocol proofs as well as requests and
responses. Counter deltas alone cannot identify a response. The simulator now
retains the latest length-valid send's parsed `WirePacketHeader`, frame length
and pre-send counter baseline per direction. The parser is the production wire
parser; a header that cannot be parsed is represented explicitly as absent.
This instrumentation neither validates/authenticates a packet nor changes
whether the virtual sink accepts it. It retains no payload or packet history.

The metadata is replaced at each new send attempt and remains after completion
or cancellation, so it must not be interpreted as an active-send record. Empty
and oversized frames are refused before metadata or counters change. A new
connection has no send observations; old endpoint activity cannot update its
replacement. Header, baseline and send-start accounting share one mutex update.
Concurrent accounting is still not a single atomic fleet-wide observation.

There is an additional fixed metadata record per direction and one header parse
per length-valid send in the host simulator. There is no shipping firmware RAM
or runtime behavior change and no performance-improvement claim.

## Matrix and exact evidence

The two new response tests cover cuts after two fragments are queued and after
two are consumed. Each runs both request directions through the mixed bridge,
both CoreBluetooth/BlueZ profile assignments and four actor schedules: cyclic
and version-one seeds 0, 7 and u64::MAX. These add 32 configurations with two
cycles each. The existing 32 request configurations now use the same stronger
header-aware boundary check.

The observer requires a new send since the scenario baseline, the requested
`Request` or `Response` context and the exact logical link address. It skips
unrelated sends rather than treating the first reverse-direction fragment as
a response. Against the selected send's own baseline it then requires:

- Exactly one started send and two queued fragments.
- No completed send and no newly reassembled whole frame.
- Zero consumed values at the queued boundary, or exactly two at the consumed
  boundary, with the expected sender/receiver actor being the last one polled.
- No saturated counters, clock movement or completed request before the cut.

An uninterrupted control pauses at each selected boundary and completes with
the exact 256-byte echoed value. The interrupted response proves the responder
has already generated a response; it does not imply the application operation
can be undone. BLE isolation then produces the exact 50-millisecond requester
timeout while independent frame-local traffic succeeds. After reconnecting,
the original logical link carries a fresh response. Actor IDs, routes, coordinated
clocks, settlement and final complete detach remain checked.

Unit tests cover parsed and unparsed headers, counter baselines, cancellation,
length refusal without mutation, immutable old snapshots, and replacement
isolation. A too-short frame remains accepted by the generic virtual sink even
though it has no Reticulum header observation.

## Limits

These are production Tokio nodes over virtual GATT profiles, not native OS
Bluetooth stacks. Response-fragment loss is not an exactly-once application
guarantee: a caller seeing a timeout cannot infer that the responder did no
work. No hardware, firmware build, native Bluetooth, Miri/ISA, exhaustive schedule
coverage or performance benchmark is claimed.

## Verification

Passed the 127 simulation library tests, all four fragment matrix tests, root
and workspace tests, simulation all-target clippy with warnings denied, and
the registered simulation suite (45.009 seconds). Repository formatting/docs
checks and `git diff --check` passed. Host Cargo runs used
`CARGO_INCREMENTAL=0`; existing ignored tests remain ignored.

```console
cargo test --locked -p prns-simulation --features controlled-time --lib --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet mixed_fragmented --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```
