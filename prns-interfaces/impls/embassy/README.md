# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

## Host-side BLE checks

The `bluetooth-auto-embassy` validation suite runs the production Embassy BLE
component tests on a normal host, including the Trouble backend, frame pools,
connection slots, and supervisor. It is registered for PR, release, and scheduled
validation tiers. PR CI and relevant pre-push checks execute it without a
connected board. To run it from the repository root:

```console
python3 validation/run.py run --suite bluetooth-auto-embassy
```

Member reads use the core's checked `receive_frame` boundary before dispatch.
A faulty source reporting a length larger than the supplied buffer follows the
existing transport-closure path instead of reaching an unchecked slice. Tests
cover exact-capacity and empty frames, invalid lengths, source failure, and
inactive slots without growing the embedded buffers.

## Duplex fanout

Each selected fanout member uses the core duplex sender: its receive side keeps
moving while its uninterrupted send waits. A local async mutex serializes delivery
into the fleet's single inbound lane. Pending forwarding retains the received
frame in that member's existing receive buffer and continues polling its send.
There are no extra packet buffers or spawned tasks; the mutex and per-peer future
state still have a resource cost that firmware builds must measure.

Receive, length, send, and forwarding failures retire the affected member without
restarting another member's send. The existing two-second fanout deadline and
disable cancellation remain in force, including while forwarding is blocked.
TX is recorded when the sink settles, so cancellation cannot erase an already
completed send. RX is recorded after shared-lane delivery settles.

This covers reception during selected members' sends, not an independent receive
pump for unselected or already-finished members. Tests use real Embassy fleet lanes
to cover concurrent peer progress, shared-lane pressure, failure isolation, and
exact accounting. This is host component evidence, not an emulated controller or
full embedded node. Native RF behavior and firmware resource evidence remain separate.
