# Dropping buffered waiters during segmented reception

macOS arm64, based on `69328eae1`, 2026-09-27.

## Scope and matrix

This extends the packet-waiter cancellation cases to requests that have already
consumed part of a real segmented file response. It does not introduce protocol
cancellation or change either runtime's orphan-completion policy.

Four tests cover sixteen combinations:

| Dimension | Cases |
| --- | --- |
| Requester | Real Tokio or Embassy buffered API |
| Cancellation boundary | After the first or second verified segment |
| Remaining transfer | Resume successfully or withhold advertisements until timeout |
| BLE profiles | ESP32/CoreBluetooth macOS or nRF52/BlueZ Linux |

The nodes exchange the ordinary 1,200-byte test file through a single 512-byte
transfer window and one incoming assembly slot. Three test-local protocol
receipts keep the abandoned request and two new packet requests live together;
the Embassy completion pool still has its ordinary two slots.

## Evidence chain

Each scenario first calibrates its exact wire boundary with a raw request. The
gate drops the next continuation advertisement, and the application must have
received exactly one or two verified segments, respectively. After loss stops,
the three complete segment values, ordering, request/link identities, file
bytes, response evidence and measured RTT are checked. Calibration records the
actual segment payload boundaries, rather than assuming metadata consumes a
fixed number of bytes in each segment.

The buffered request then reaches the same boundary. `select!` owns and drops
its future when the continuation advertisement is lost. No partial response may
have reached the raw application while the buffered caller was alive. Two new
buffered echoes run concurrently on that same link while loss continues. Both
must return their own complete bytes and measured RTT; this also proves both
Embassy completion slots are immediately reusable without evicting the old
protocol receipt.

Only after both echoes finish is the new, opt-in responder probe armed. It
retains one real `Respond` settlement, including command identity and observation
time, rejects overwrite/rearm, and ignores traffic while idle. Its unit tests
cover observation before/after waiting, irrelevant settlement types, reuse and
cancelling the observation future. No packet payload or production state is
retained by this probe.

When advertisements resume, Embassy must expose exactly the unconsumed suffix
and one successful settlement for the abandoned request. Previously buffered
prefix bytes must not be replayed to the application. When loss continues,
Embassy must instead expose just one `Timeout`, without additional segments.
Tokio keeps the abandoned response and terminal journal private in both cases.

Sender timeout is not receiver timeout. A first version of the test tried to
reuse the receiver immediately after the sender failed and correctly encountered
its still-live assembly. The receiver has its own between-segment deadline.
Embassy tests now await both actual settlements. Tokio's receiver settlement is
private, so its timeout case instead tests reclamation within a fixed 20-second
virtual-time window from request start. That is a bounded eventual-cleanup
assertion, not evidence for the exact private settlement instant. Sender
settlement is still observed directly, not inferred from that window.

Afterward a different, already-established link must acquire the one assembly
and transfer slot. The complete raw/refusal/buffered recovery sequence then runs
on both spare and original links, with another concurrent completion-slot reuse
check. Neither nodes nor links are reset; BLE stays connected, and bounded
journals, gates, probes and radio ownership must be clean at teardown.

## Diagnostic and limits

Temporarily omitting `retire_response_assembly` from the shared request-timeout
settlement made both new timeout tests fail during spare-link reuse with
`ResponseTransferFailed(TransferCorrupt)` instead of the expected ordinary
size refusal. Each test stops at its first failing boundary/profile; this is a
focused diagnostic, not an exhaustive mutation campaign. The production line
was restored exactly before final verification.

Production behavior, capacities, memory layout and wire formats are unchanged.
These are host-side real-node simulations over virtual GATT, not native OS
Bluetooth controller tests, physical RF qualification or firmware execution.
They do not prove immediate protocol cancellation, request-admission
cancellation, indefinite Tokio retention bounds, arbitrary worker scheduling or
full byte-for-byte replay. Independent conflicting whole-Resource peer
injection remains a separate gap.

## Verification

Passed on macOS arm64, with host Cargo runs using `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble abandoned_segments --quiet`.
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet` (84 tests).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware, Miri/ISA, physical hardware, stock interoperability or benchmark
rerun is claimed for this test-only slice.
