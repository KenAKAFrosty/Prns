# Interrupted segmented responses through virtual BLE

Local macOS arm64 evidence, 2026-09-26, candidate based on `d571e6b63`.

## Scenario and detection scope

This slice adds a controlled radio outage to the
[successful segmented-response scenarios](segmented-resource-simulation.md).
It does not change production code. Once the harness used an explicit mutable
topology and a deadline beyond the production response grace, both scenarios
passed against the existing implementation.

Each ESP32/Apple and nRF52/BlueZ pairing performs the following twice, first with
Tokio requesting from Embassy, then with Embassy requesting from Tokio:

1. Issue a raw request for the existing 1,200-byte static file. The single
   512-byte Resource window forces three segments through real production
   request handlers, segmentation, encryption, GATT framing and completion.
2. Await the first application-visible segment. Assert its entire value:
   expected link, observed request ID, index one of three, and an exact literal
   file prefix. It must be nonempty and fit the transfer window. No second event
   may already be waiting.
3. Isolate the two virtual radio addresses in that same awaiting actor, before
   it returns or the harness performs its normal post-operation settlement.
   The one active BLE connection closes immediately and stays absent until
   reachability is explicitly restored.
4. Require exactly one terminal `SendRequestFailure::Timeout` for the issued
   command. No further segment, whole response or successful settlement may
   appear. The Embassy requester's separate settlement inventory must agree.
5. Restore reachability and let the production supervisors rediscover each
   other. Both old Reticulum links must report `LinkClosedReason::Timeout` on
   both nodes. The observed Embassy responder has no terminal result at request
   expiry, then reports exactly `RespondFailure::Resource(LinkClosed)` when its
   link retires.
6. Establish two fresh links and rerun the complete zero-budget refusal, raw
   three-segment success and repeated buffered-success sequence on the same
   nodes and single-slot storage. Old/new link IDs differ; all returned file
   bytes and measured RTTs must match. No extra raw settlements, closures or plain
   deliveries may remain. Final actor teardown closes all connections and
   detaches both radios exactly once, with no discarded medium trace events.

The raw observer gains only a notification-driven, one-event-at-a-time read.
Its eight-event and 2 KiB-per-body limits remain unchanged. Focused tests check
FIFO consumption and wakeup after an already-pending read, with bounded paused
Tokio time. It still neither intercepts nor implements buffered request results.

Each interruption operation has a 20-second controlled-time deadline rather
than the success harness's ten seconds. The normal inter-segment request grace
exceeds ten seconds; production timers are not shortened for the test. Existing
one-millisecond stepping, 256 polls per operation tick, 128 settlement polls,
eight actor slots and 60-second discovery deadlines remain in force. These two
longer scenarios explicitly reserve a 131,072-event BLE trace; ordinary success
scenarios retain 4,096 events. Every trace still fails the test on eviction.

## Verification

Host Cargo commands used `CARGO_INCREMENTAL=0`. Passed:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble interrupted`
  (two scenarios, followed by ten consecutive runs: twenty more executions).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble response_trace`
  (five tests), and the full `embassy_ble` integration binary (23 tests).
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- Root formatting, whitespace and touched-document relative-link checks.

## Coverage limits

This is an honest-peer virtual-radio interruption after one verified segment,
not corrupted, duplicated or malicious wire input. Failure assertions concern
raw requests; buffered futures are exercised for successful recovery only.
Recovery waits for old-link retirement, so it does not establish prompt assembly
cleanup on a still-live old link. Exact cumulative value accounting, compression,
reboots, later-segment fault timing and buffered partial-result disposal remain
separate work in the [response-size investigation](../../../prns-core/plans/response-size-accounting.md).

The clock and actor polling are controlled, but production Tokio entropy and
boot origins remain uncontrolled. No byte-for-byte replay or fixed latency is
claimed. Endpoint labels exercise shared protocol paths, not native platform
Bluetooth APIs, RF, board firmware or a many-node memory/throughput workload.
No shipping runtime, core policy, storage profile, wire format or RAM budget
changes. No mutation owner changes, and no new mutation, stock-peer interop,
firmware/resource, Miri/ISA or hardware runs are claimed.
