# Partial deadline batches across repeated BLE recovery

macOS arm64, based on `2a478971a`, 2026-09-27.

These three slices combine equal and distinct deadlines in the same fleet.
Three callers are admitted at separate times and lose alternating partial
requests/responses across separate BLE reconnects. The first and last callers
share a deadline; the middle caller has another. All recovery and pre-expiry
fresh traffic finish before the earliest deadline.

## Slice 1: pair before singleton

Admission-order deadlines are batch origin plus `[50, 70, 50]` milliseconds.
At 50 milliseconds the first and last callers must complete together, leaving
the middle caller pending. Fresh concurrent responses and frame-local traffic
succeed before that caller alone expires at 70 milliseconds. Live actor counts
fall from six to four to three.

## Slice 2: singleton before pair

The deadlines are `[70, 50, 70]`. The middle caller alone expires first; the
outer pair stays pending through fresh traffic and then completes together.
Live actor counts fall from six to five to three. Pair membership is not
contiguous in admission order.

## Slice 3: cancel one paired caller

Using `[50, 70, 50]`, the last caller is cancelled after recovery. The first
caller must still expire at 50 milliseconds and the middle caller at 70.
The cancelled member never appears in either completion batch. Cancellation
leaves the clock and BLE activity unchanged; counts fall from six to five on
cancellation, then to four and three at the deadlines.

## Coverage and limits

Each boundary compares the whole completion map, checks exact live actor
counts and confirms retired caller IDs report `NotLive`. No early completions
are permitted. Original node actors, clock agreement, continued traffic and
clean teardown remain checked. The deadline owner now has a grouped-case
child module instead of growing a single leaf file.

Each slice has two tests spanning queued/consumed fragments, both alternating
loss orders, both request directions, both virtual GATT profile assignments
and four actor schedules (cyclic, seeds 0, 7, u64::MAX). That adds 64
configurations per slice, 192 total, each with two three-outage batches.
The existing eight-actor budget is unchanged.

Production impact: none. These cases test existing production request
behavior over virtual BLE. They do not establish native Bluetooth, firmware,
hardware, private storage reclamation, performance or exhaustive scheduling
guarantees. Equal-deadline completion means one same-tick actor drain, not one
poll or an atomic operation inside production code.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet flapping --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
git diff --check
```

All 36 focused flapping tests, all-target clippy, root/workspace tests,
repository formatting/docs checks and `git diff --check` passed. The
registered simulation suite passed in 73.837 seconds. Existing ignored tests
remain ignored. Firmware, hardware and other-host lanes were not run for these
host-test-only changes.
