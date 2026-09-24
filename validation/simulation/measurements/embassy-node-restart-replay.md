# Embassy node restart replay

Earlier Embassy-only byte replay covered boot, traffic and teardown; mixed-runtime
replay added reconnect without reconstructing a node. This follow-up replaces one
complete Embassy receiver while its sender and shared timeline remain live.

## Fixture ownership

Each Embassy node fixture now retains its runner task ID. Consuming `stop` removes
that actor through the existing shared runner cancellation API. The Embassy adapter
checks the complete clock snapshot is unchanged by cancellation. A second cancel
of the retired ID reports `NotLive`. The old node future and its BLE supervisor
are dropped; the scenario's process-global clock lease remains held throughout.

The replacement receives fresh command/completion storage, entropy ownership,
node state and supervisor, but the same configured identity and radio address.
No new runtime API or unsafe lifetime manipulation is needed. Static fixture
objects remain allocated until process exit, as in the isolated memory checkpoint;
this is not a reclaimable-static-storage or repeated-lifecycle memory claim.

## Scenario and assertions

- Two real Embassy nodes discover, announce, establish a protocol link and echo
  a 256-byte request through the virtual BLE medium.
- At seven simulated milliseconds, only the receiver actor is canceled. The
  sender loses membership, the BLE connection closes, and no additional values
  enter the old wire capture. The clocks do not reset or move during cancellation.
- A receiver reconstructed at the same address discovers and establishes a new
  protocol link. The old and new BLE phases must have distinct connection IDs.
- A retained old command handle can enqueue an announcement into its old static
  channel, but cannot produce traffic through the replacement actor. This is
  isolation evidence, not a claim that static handles detect a stopped owner.
- An explicit request on the sender's old protocol link returns `Timeout` after
  exactly 50 simulated milliseconds. The replacement records no command
  settlement from responding to it. A request on the fresh link then succeeds.
- Final teardown leaves zero active BLE connections, and the complete attached
  radio inventory equals the detached inventory for all three node incarnations.

The old-link timeout is intentional evidence of the observed contract: a quick
peer restart can leave protocol-link state in the surviving sender even after
the radio reconnects. Immediate `NoSuchLink` rejection is not assumed. The test
proves obsolete traffic cannot successfully cross into the replacement, settles
within its explicit budget and does not prevent new traffic. No production bug
or behavior change is inferred from that distinction.

## Replay controls and limits

The receiver uses ESP32 and nRF52 protocol profiles in separate cases. Each case
compares four fresh whole transcripts, retaining every accepted wire value,
discovery event, reconnection boundary and response. Changing only replacement
entropy must preserve the complete pre-restart wire prefix while changing later
traffic and preserving responses. Changing the payload must change both response
values and traffic. All captures remain bounded with zero eviction.

This is one receiver reconstruction in a two-node scenario with cyclic actor
scheduling and inline crypto. Dropping futures runs Rust destructors: it does
not emulate abrupt physical power loss, MCU startup, flash, native radio teardown,
or persistence recovery. Repeated restart waves, larger Embassy fleets and
process-isolated lifecycle memory measurements remain separate milestones.

Production impact: none. Changes are confined to simulation fixtures and tests;
shared cancellation, Embassy timebase and entropy APIs are reused unchanged.

## Verification

Host: macOS arm64. Cargo commands use `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble replay::restart --quiet
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

The focused Embassy target passes 93 tests, and the registered simulation suite
passed in 63.243 seconds. The restart matrix also passed in three additional fresh
test processes. Firmware, physical devices, other
operating systems and embedded ISA execution are not implied by these host checks.
