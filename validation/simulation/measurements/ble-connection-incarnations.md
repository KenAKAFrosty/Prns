# BLE connection incarnations and reconnect evidence

## Capture identity

Every enabled captured connection now gets a typed ID from its capture owner.
Both directions and both characteristics share it. Reused addresses, retained
old links, capture clones, and ring eviction cannot recycle it. IDs are local
to one capture owner, not globally unique or durable device identities. Failed
admission can consume an ID; contiguity is not promised. A checked counter
refuses allocation at exhaustion, and the virtual backend reports
`DialOutcome::InvariantViolation` before creating another link. Capture-disabled
labs do not allocate IDs.

The owner retains only the next counter, not a history of connections. Enabled
capture adds one allocation-time lock and an ID in each captured connection and
value. Snapshot copying and retention remain explicitly bounded by the caller.
This is simulator overhead, not firmware RAM or a shipping wire-format change.

Focused tests cover exhaustion without wraparound, whole-value eviction and
snapshot independence, and old queued controls across same-address replacement.
Old sends/receives remain closed, replacement directions share only the new ID,
and dropping retained old links cannot close or relabel the replacement.

## Real-node recovery

The two-node replay fixture now has a partition/reconnect lifecycle. It proves:

- A successful fragmented request/response before and after the partition.
- Exact peer inventory removal while isolated, and restoration after recovery.
- No new accepted values from the retired connection during isolation.
- Distinct IDs across the two connections despite identical addresses.
- Exactly one valid Hello/Welcome pair per connection, with exact expected
  identity, endpoint, capability, RSSI and group bytes for the selected initiator.
- Identical data-channel values across nine fresh recovery runs and differing
  data-channel bytes under a changed host seed with the same application result.
- No capture or discovery overflow, no dropped discovery observations, and
  complete actor/connection/radio cleanup.

## What the attempted stronger test found

The first test required equality of the entire raw reconnect transcript. It
failed: the first control value after the reconnect boundary (value 94 in this
fixture) could be a Hello from either address. Advertising/scanning event order
could differ too. Both scenarios recovered and exchanged the correct response.

The outer manual actor order is not the complete execution schedule. The Tokio
BluetoothAuto supervisor uses fair `tokio::select!` arbitration among backend,
handshake, close and status inputs. Simultaneously ready branches are not
controlled by the three node-owned entropy providers. This is an uncovered
replay input, not evidence of a broken Bluetooth handshake or lost traffic.

The test still retains the raw transcript. It does **not** sort control messages,
rewrite identities or assert whole reconnect transcript equality. Data values
are compared in their original order as whole values, including direction and
connection identity; controls are validated against protocol expectations for
the actual selected initiator. Fresh-connect tests retain their existing exact
whole-transcript comparison. None of this establishes arbitrary scheduler,
restart, native radio-stack or RF replay.

The next step is a deliberate arbitration-input design, preserving shipping
fairness and making competing readiness reproducible in validation. Fixed
priority or a process-global RNG override solely to make this test pass would
not be an appropriate shortcut.

The subsequent [arbitration follow-up](ble-replay-arbitration.md) closes this
specific gap and refines the diagnosis: both the enclosing interface driver and
the BLE supervisor need controlled inputs for full reconnect transcript replay.

## Production behavior

Unchanged. This round is entirely simulation capture, fixtures and evidence.
The backend and replay modules now use directory module roots because they own
separately housed tests and recovery helpers.

## Verification

Passed on macOS arm64; host Cargo invocations used `CARGO_INCREMENTAL=0`.

```console
cargo test -p prns-simulation --lib capture --quiet
cargo test -p prns-simulation --features controlled-time --test manual_fleet ble_replay --quiet
cargo clippy -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

Six focused capture tests and three replay/recovery tests passed. A 12-process
repeat of the replay/recovery tests also passed before the final exact greeting
assertions were added. The final registered simulation suite passed in 85.399
seconds, including 133 library tests and 96 manual-fleet tests; the existing
opt-in memory probe remained ignored. Root workspace tests include the ordinary
root test path. No native platform suites, firmware builds, hardware smoke tests
or performance benchmarks were run for this simulation-only change.
