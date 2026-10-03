# Deadline-edge retirement and complete caller abandonment

macOS arm64, based on `a9ea74ea5`, 2026-09-27.

Three review slices extend the alternating-loss fleet with reverse absolute
deadlines: the first, middle and last admissions target batch origin plus
90, 70 and 50 milliseconds respectively. All three requests suffer partial
request/response loss across separate BLE reconnects before retirement.

## Slice 1: cancel the earliest deadline

The last-admitted caller is cancelled after recovery. Its 50-millisecond
boundary must remain silent, while the two surviving callers stay live and
expire individually at 70 and 90 milliseconds. This tests removal at the front
of deadline order rather than just the middle of admission order.

## Slice 2: cancel the latest deadline

The first-admitted caller is cancelled instead. The remaining two callers
expire at 50 and 70 milliseconds, and the former 90-millisecond boundary is
silent. Fresh traffic succeeds after each boundary, including after all live
old callers have retired but before the cancelled caller's former deadline.

## Slice 3: abandon all pending callers

All three callers are cancelled in admission order. Each cancellation returns
`Cancelled`, an immediate retry returns `NotLive`, and the live actor count
decreases by exactly one. Only the three production node actors remain.
Every former deadline must be silent, and fresh concurrent responses plus
frame-local traffic succeed between boundaries. This observes caller-visible
retirement and continued service, not reclamation of private node storage.

## Common assertions and coverage

Cancellation changes neither the clock nor the BLE activity snapshot. Each
deadline compares the whole completion map and exact actor count. Original
node task IDs, clock agreement and complete medium teardown remain checked.
Retirement now owns a child module for these cases; existing middle-caller
and deadline-tick cancellation tests retain their behavior.

Each slice has two tests covering queued/consumed fragments, both alternating
loss orders, both request directions, both virtual GATT profile assignments
and four actor schedules (cyclic, seeds 0, 7, u64::MAX). That is 64
configurations per slice, 192 total, each with two three-outage batches. The
existing eight-actor budget is unchanged.

Production impact: none. This is host simulation coverage of existing request
behavior, not native Bluetooth, firmware, hardware, exhaustive scheduling or
full byte-replay evidence. The all-cancelled case uses distinct reverse
deadlines, not the shared-deadline cancellation boundary from the prior slice.

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

All 30 focused flapping tests, all-target clippy, root/workspace tests,
repository formatting/docs checks and `git diff --check` passed. The
registered simulation suite passed in 64.983 seconds. Existing ignored tests
remain ignored. Firmware, hardware and other-host lanes were not run for these
host-test-only changes.
