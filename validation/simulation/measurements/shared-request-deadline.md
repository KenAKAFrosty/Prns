# Exact shared-deadline settlement after repeated BLE loss

macOS arm64, based on `ffa01b273`, 2026-09-27.

## Review slice 2

Three callers start at distinct virtual times but receive different timeout
durations targeting the same absolute deadline: batch origin plus 50
milliseconds. Each caller loses a selected partial request or response across
a separate BLE outage. Recovery and fresh traffic finish while all three
callers remain pending.

At the shared deadline, draining runnable actors must produce exactly the
three expected timeout completions, with no caller completing early or left
for a later tick. The live actor count falls from six to three, and all three
caller task IDs report `NotLive` when cancellation is attempted afterward.
Fresh concurrent responses, local traffic, clocks and clean teardown remain
checked. This is one same-tick drain, not a claim that futures complete in one
poll or atomically inside production code.

Four tests cover alternating loss orders and queued/consumed boundaries. The
same directions, GATT profile assignments and four actor schedules as the
reverse-deadline slice yield another 64 configurations, each with two batches.
Together these slices add eight tests and 128 configurations. They do not add
simultaneous-expiry cancellation cases or native Bluetooth coverage.

Production impact: none. Existing production timeout behavior passed these
new checks without changes. The fixed eight-actor fleet budget is unchanged.

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

All 20 focused flapping tests, clippy, root/workspace tests and repository
formatting/docs checks passed. The registered simulation suite passed in
49.879 seconds. Existing ignored tests remain ignored. `git diff --check`
passed. Firmware, hardware and other-host lanes were not run for these
host-test-only changes.
