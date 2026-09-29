# Selective actor teardown and host-node restart

macOS arm64, based on `2f6aa63fe`, 2026-09-27.

## Capability and ownership

`ManualTaskRunner::cancel` removes one admitted actor without polling it or
advancing time. It returns `Cancelled` or `NotLive`, releases the actor's capacity,
and never reassigns its ID. IDs are scoped to their admitting runner; they are not
portable handles between runners. Completion and cancellation retain no output
queue or unbounded history of retired IDs.

The actor leaves both ownership and readiness indexes before destruction. The
ready-index lock is released before user destructors run, so a destructor may
wake a surviving actor without deadlocking. Its own stale wakers cannot restore
it or wake a newly admitted replacement. Destruction enters the paused Tokio
runtime context, matching ordinary actor completion.

Clock and spawned-task guards run before mutation and after destruction. A
precondition failure preserves actor ownership; a post-destruction failure
reports the violation but cannot undo effects or restore the future. Such a
runner must be discarded, as with existing post-poll failures. Destructors must
remain nonblocking and must not spawn work outside the controlled actors.

Eight unit tests cover cancellation before first poll, exactly-once destruction,
capacity reuse, completed/already-cancelled IDs, timer retirement, self/stale
wakes, destructor-driven sibling wakeup, precondition refusal, post-destruction
spawn refusal, ID exhaustion, and sparse cancellation among 1,024 actors. The
scale test checks survivor ordering and bounded live/ready indexes; it is not a
1,024-node simulation or a throughput measurement.

## Real-node scenarios

Two cases use four real Tokio `PrnsNode`s in two isolated pairs, with four live
endpoint slots, eight actor slots, eight-entry receive queues, eight delayed
delivery slots and a 4,096-event trace. Existing inline crypto keeps execution
inside the controlled runner. No production clock, node or interface code is
replaced with a fake.

The first schedules an actual announcement for delivery five milliseconds later,
then cancels its intended receiver. Rebuilding that node with the same identity
and interface tag gives a fresh actor ID and medium endpoint ID while preserving
the logical interface ID. The old delayed delivery remains bound to the old
endpoint and yields exactly one `EndpointClosed` trace event, never delivery to
the replacement. A fresh announcement, link and exact echo then succeed. The
other pair exchanges before and after the delayed delivery on its original link.

The second stops at an observed outgoing Request packet, before the receiver's
next actor poll. Cancellation drops that receiver's queued ingress; it does not
invent a response or a graceful-stop completion. The remote request stays
pending through 49 milliseconds and returns exactly `Timeout` at 50. The other
pair exchanges during downtime. A rebuilt receiver establishes a different link
and answers fresh requests while the unaffected link remains usable. Clock
observations show 50 milliseconds elapsed for a survivor and zero since boot for
the replacement, on the same unchanged simulation timeline.

Both scenarios keep the other three node actors alive. End-of-scenario orderly
shutdown completes the four current actors, and every old/new interface detaches
exactly once. No actors, delayed deliveries or truncated trace entries remain.
The second test has no medium reception-drop event: its frame reached the old
ingress queue and was discarded with that owner, not dropped by the medium.

## Diagnostic and limits

Temporarily omitting cancellation's ready-index retirement made the focused
stale-waker test fail by trying to poll a removed actor. The retirement was
restored before final verification. This checks the new scheduler invariant;
it is not an exhaustive mutation campaign or a discovered shipping-code defect.

This is a simulator capability change only. Shipping behavior, firmware memory
budgets, runtime cancellation policies and wire formats are unchanged.

Dropping a Rust actor runs its destructors. It is deliberately not described as
physical power loss: hardware interrupts, DMA, partial flash writes and volatile
peripheral state are not modeled. Node recipes restore test-provided identity;
they do not read persisted device state. BLE controller reset, routed transport
restart, restart during persistence, pooled workers and complete entropy/byte
replay remain separate work. Only endpoint nodes on virtual frame interfaces
are restarted by these scenarios.

## Verification

Passed on macOS arm64; host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --lib cancellation --quiet` (eight tests).
- `cargo test --locked -p prns-simulation --features controlled-time --lib --quiet` (100 tests).
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet` (five tests).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

Firmware, Miri/ISA, hardware smoke tests, native OS Bluetooth stacks and benchmarks
are not rerun for this simulator-only change.
