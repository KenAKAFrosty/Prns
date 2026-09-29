# Explicit readiness arbitration closes the observed BLE replay gap

## Diagnosis and fix

The previous round's reconnect capture could show a Hello from either peer at
the same connection boundary. Controlling protocol entropy and the outer manual
actor order did not control the order in which nested runtime events were polled.

This round first introduced per-supervisor event selection. A short test passed,
but a broader repeated test reproduced the original differing Hello at wire
value 94. Temporarily forcing the interface driver's message-first polling made
that test pass; leaving the BLE supervisor uncontrolled still failed. That
diagnostic fixed-priority change was removed. The final implementation controls
both owners explicitly, with rotating selection in the fixture and normal
Tokio fairness retained by ordinary construction.

- `PrnsNode::with_interface_arbitration` selects node-owned arbitration between
  interface messages and task completion. `TokioFair` is the existing default;
  `RoundRobin` requires a named first source, advances after a successful choice,
  and polls the other source when the preferred source is pending.
- `BluetoothAuto::with_event_selector` supplies an instance-owned selector over
  the five typed supervisor event futures. Its default is a zero-sized,
  statically dispatched Tokio fair selector. The fixture supplies its own
  rotating selector with an explicit first branch. It cannot manufacture the
  supervisor's private event values or change the protocol entropy source.
- Both selectors retain wake registration and cancellation behavior. No global
  RNG override, wall-clock sleep, event sorting, or captured-byte rewriting is
  used. Empty/exhausted sources stay pending for that selection turn.

The interface-driver policy belongs to the common Tokio runtime, so other
interfaces can reuse it. The BLE event boundary belongs to the Tokio Bluetooth
adapter. Neither belongs in the no-std protocol core: these changes control
executor readiness, not a shared protocol rule or radio effect.

## Evidence

The reconnect test again compares the **entire** raw transcript: discovery
events in original order, all control values, all encoded GATT fragments,
connection IDs, checkpoint boundaries and application bytes. The first unequal
wire value or discovery event is reported before the whole-value assertion for
useful diagnostics; nothing is removed from the comparison.

The test covers both initial interface-driver choices crossed with all five
initial supervisor choices. Each of those ten combinations runs nine times and
must reproduce its own transcript. A separate repeated reconnect test retains
the changed-host-seed control: wire data must change while the application
response stays the same. Existing changed-payload and fresh-connection checks
remain. Whole greeting expectations, bounded retention and complete cleanup
remain enforced.

Owner tests cover fair-default source wakeup, cancellation of borrowed sources,
rotating ready-source fairness, pending-source bypass and cancellation without
cursor advancement. The fixture selector is also tested for all-ready rotation
and pending wake delivery. Twelve separate test-process runs passed the full
replay test group, including the ten-order matrix.

## Production impact and limits

Normal construction remains Tokio-fair at both changed boundaries. No fixed
priority replaces production fairness; there is no BLE wire-format or
authorization change. The new interface-driver policy is one small enum per
node. Default BLE selection adds no owned allocation or seed; the fixture
selector uses stack-pinned futures and a cursor. This is not a measured
throughput, future-size or zero-cost claim, and no benchmark was run.

This closes the **observed two-node reconnect replay gap**. It does not claim
determinism for every Tokio selector, interface count, worker completion,
multi-threaded executor, native Bluetooth stack, RF condition or firmware. Other
internal schedules must be audited when scenarios exercise them. In particular,
this pass does not alter peer send/receive or interface-stop arbitration.

## Verification

Passed on macOS arm64; host Cargo invocations used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto-runtime --lib event_selection --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib arbitration --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay --quiet
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto --lib --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --all-targets -- -D warnings
cargo clippy --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto --all-targets -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
python3 validation/run.py run --suite integration-capstones
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

The focused replay group passed six tests and was repeated in twelve separate
test processes. Tokio interface coverage passed 17 tests; the runtime passed
284 tests plus one doctest, leaving the existing timing probe ignored. The
registered simulator suite passed in 101.401 seconds with 99 manual-fleet tests;
integration capstones passed in 41.859 seconds with loopback socket access.
Root workspace coverage includes the ordinary root test path. No configured
mutation owner changed. Firmware, hardware, other OS targets and benchmarks were
not run; native macOS dependencies compiled, but no physical Bluetooth smoke
test was performed.
