# Concurrent paired-fleet BLE replay

## Question and workload

Does the explicit arbitration introduced for one BLE pair still produce the
same captured traffic when several independent links make progress together,
one pair disconnects, and outer actor order is seeded differently?

Eight real Tokio nodes form four isolated BLE pairs. Each node owns its
explicit entropy sources and rotating interface/BLE arbitration. The fixture
varies the first arbitration choices by node, uses 20-byte GATT values, and
admits all four link-establishment actors before settling. It then admits four
256-byte requests together, with a distinct sender marker in each payload.

The first pair is isolated. Both peer inventories disappear and its connection
closes before recovery. The remaining three pairs complete another concurrent
request batch using their original Reticulum links. After reachability returns,
only the affected pair establishes a replacement link. All four pairs then
complete a final concurrent request batch.

The capture proves that the three unaffected pairs retain their original BLE
connection IDs, whereas the recovered pair uses a distinct ID at the same
addresses. No values are accepted from the retired pair while isolated.
Direction/address pairing is checked, so unrelated peers cannot silently appear
in the transcript. All response values are validated and retained by sender;
wire and discovery events remain in their original order.

## Replay evidence

For each outer schedule—cyclic, seed 0, seed 7, and seed `u64::MAX`—four fresh
runs compare complete transcripts, including every control/data value, direction,
connection ID, discovery event, phase boundary and application response. Each
schedule must repeat itself; different schedules need not produce identical
interleavings. The scheduler's existing versioned permutation is reused.

Changed host entropy must change captured wire values without changing the
application responses. An equal-length changed payload must change both. The
full BLE replay group, including these cases and the preceding two-node tests,
also passed twelve separate test-process runs.

Every run rejects wire-capture eviction, discovery-trace eviction and dropped
observations, then proves all node actors, active connections and radio
registrations are retired. Actor capacity is twelve, radio capacity eight,
capture capacity 16,384 values and discovery capacity 262,144 events. Peers have
one connection each; these are explicit bounded fixtures, not resource benchmarks.

## Scope and production impact

No production behavior changed and no additional scheduling control was needed.
The two- and eight-node fixtures now share a named `LiveNode` construction helper;
the simulation does not duplicate the Bluetooth protocol or runtime executor.

This is four independent pairs, not eight mutually connected nodes, routed mesh,
multi-peer supervisor coverage, native OS Bluetooth, RF, or firmware evidence.
It does not establish thousands-of-node scalability, arbitrary interleavings,
worker-completion determinism or long-duration clock acceleration. Those remain
separate roadmap work. No new production defect was discovered by this slice.

## Verification

Passed on macOS arm64; host Cargo invocations used `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

Eight focused replay tests passed, including the two new fleet cases. The
registered simulator suite passed in 82.459 seconds with 101 manual-fleet tests;
the existing opt-in memory probe remained ignored. After adding explicit actor
count assertions for concurrent admission, the focused replay group and Clippy
were rerun successfully. Root workspace coverage includes the ordinary root
test path. Firmware builds, hardware, other OS targets, integration capstones
and performance benchmarks were not rerun for this test-only slice.
