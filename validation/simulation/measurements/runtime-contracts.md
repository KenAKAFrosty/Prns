# Shared runtime behavioral contracts

`embassy_ble/contracts/` runs one scenario against Tokio/Tokio, Embassy/Embassy,
Embassy/Tokio and Tokio/Embassy. Each pairing uses cyclic and seeded outer actor
order. It lives beside the existing shared-clock fixtures; the Tokio-only case
does not construct an Embassy node. This is a first reusable behavioral contract,
not a claim that all existing scenario families have been consolidated.

The private adapter selects real production nodes and translates their public
APIs. It converts Embassy's bounded response buffer to a host vector without
normalizing errors, payloads or RTTs. Scenario assertions do not branch on runtime.
The expected report is constructed independently, rather than treating either
implementation as the oracle for the other.

## Contract

- Both peers discover the expected interface identities and settle to one BLE
  connection before establishing distinct links in each direction.
- Concurrent 256-byte echoes return complete payloads and zero simulated RTT.
- Dropping exactly one response per direction produces the original typed
  `Timeout` at exactly 50 ms, individually and as a concurrent batch.
- Fresh requests succeed on both original links after expiry.
- Each direction cancels two requests after observing their deliberately lost
  responses. Fresh requests then succeed without advancing time to clear them.
- A partition removes connected membership and closes the active connection.
  Reconnection establishes fresh protocol links and resumes bidirectional echo;
  no retired captured connection identity carries recovery traffic.
- Teardown balances both radio attachments and leaves zero active connections.
  Wire and discovery traces must not have evicted any entries.

Fixture-owned bounded settlement/closure diagnostic recordings are drained
between exchanges. The scenario does not edit production queues or request state
to make the next phase succeed. Existing detailed settlement regressions retain
their own diagnostic assertions.

## Timing, replay and scope

Discovery and recovery have a three-advertisement-interval horizon and a bounded
number of wake-driven steps. The fixture advertises every 60 simulated seconds;
that horizon is a test bound, not a shipping reconnect-latency promise. During
development, one mixed orientation recovered at tick 120,000 rather than 60,000.
The cause of that extra cycle has not been established. Recovery elapsed time is
kept separate from the exact semantic report: latency parity is not claimed.

Tokio uses its ordinary entropy source and internal fairness here. The seed
selects only outer actor order. Wire bytes and transient candidate-connection
counts can differ; settled behavior must satisfy the same contract. Existing
explicit-entropy complete-byte replay tests remain separate and unchanged.

This covers two real nodes on a virtual GATT medium, not native Bluetooth stacks,
RF, firmware execution, persistent reboot, Resource transfer or large-scale
capacity. Platform endpoint labels remain handshake metadata. Future generated
campaigns can reuse the adapter without copying a Tokio-only scenario into an
Embassy-only counterpart.

Production impact: none. All changes are host-only tests and documentation;
shipping behavior, firmware memory and public APIs are unchanged.

The subsequent [generated campaign](generated-runtime-contracts.md) exposed and
fixed obsolete Tokio BLE close notifications affecting replacement connections.
That follow-up changes production adapter behavior; the original contract
foundation described here did not. Identical recovery latency remains unclaimed.

## Verification

Run Cargo commands with `CARGO_INCREMENTAL=0`:

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble contracts:: --quiet
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
python3 validation/run.py run --suite virtual-device-simulation
cargo test --workspace --locked --quiet
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

On macOS arm64, five consecutive fresh-process runs of the eight-case contract
matrix passed. The full shared-clock target passed 99 tests; the registered
simulation suite passed in 66.28 seconds. These are host test results, not hardware
qualification or a performance benchmark.
