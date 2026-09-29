# Stale-link teardown clears dependent channel wakes

macOS arm64, based on `6c8200cc2`, 2026-09-26.

## Scope

Test-only shared-core assurance following the mixed-runtime
[blocked-send finding](blocked-send-link-wakes.md). That scenario exposed stale
receipt scheduling; channel and pairing refreshes were included by inspection
because link retirement owns those operations too. This slice directly exercises
a live channel send alongside a pending request through the real link-deadline
entry point. It is an engine fixture, not a new mixed-runtime simulation.

The test derives stale-link expiry from the active link's timing parameters and
places both operation deadlines later. Teardown must settle both commands exactly
once as `LinkClosed`, report one timeout closure, emit the exact encrypted close
frame, and remove the link, channel and receipt. Applying the returned wake delta
to the previously cached schedules must equal a fresh whole-engine recomputation.
Subsequent link, receipt and channel deadline calls must emit nothing and consume
no entropy.

The complete delta is checked too, but this does not establish live pairing or
Resource teardown coverage: those stores are empty in this fixture. There is no
production behavior, allocation, capacity or wire-format change.

## Diagnostic refusal

Temporarily removing only `channel_timeouts: self.channel_timeouts_wake()` from
`fire_due_link_deadlines` made the focused test fail its whole-schedule comparison:
the cached channel wake remained `At(InstantMillis(119856))`, while recomputation
was `Idle`. Restoring the refresh restores the test. This is a targeted manual
mutation, not a full mutation-campaign result.

## Verification

Passed on macOS arm64, with `CARGO_INCREMENTAL=0` for Cargo tests and clippy:

- `cargo test --locked -p prns-core stale_link_teardown_disarms_channel_and_receipt_wakes_exactly_once`.
- `cargo test --locked -p prns-core engine::deadlines --quiet` (40 tests).
- `cargo test --locked --quiet`.
- `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core --all-targets -- -D warnings`.
- `cargo fmt --all -- --check` and `git diff --check`.

The complete pre-push ladder, including 16 firmware targets, Miri and the three
target-ISA lanes, passed for the committed checkpoint `6c8200cc2` before this
test-only addition. Those platform checks and the mixed-runtime suite were not
rerun for this slice; no new hardware or performance evidence is claimed.
