# Stale-link teardown preserves exact controller-pairing scheduling

macOS arm64, based on `f8fd4230e`, 2026-09-26.

## Scope

This test-only slice follows the mixed-runtime
[blocked-send wake finding](blocked-send-link-wakes.md) and the direct
[channel-wake assurance](link-teardown-channel-wakes.md). The original fix also
refreshed Remote Control pairing scheduling because link retirement can abort a
pairing attempt. This adds direct evidence for that dependency.

The shared-core fixture has two active links: one reaching its stale deadline,
and one recently activated. A controller pairing attempt awaiting an offer is
bound to either link, with a deadline after stale teardown. The real
`fire_due_link_deadlines` entry point must:

- Abort the attempt exactly once when its own link expires, clearing its wake.
- Preserve the full awaiting-offer state and deadline on the unrelated fresh link.
- Close only the stale link, with one exact encrypted close frame and timeout journal.
- Return a delta that makes the cached whole-engine schedule equal recomputation.
- Emit nothing when called again at the same time, including through the pairing
  deadline entry point; neither call may request entropy.

This is a deterministic engine fixture with directly prepared state, not a
mixed-runtime pairing exchange or an authenticated-handshake test. Later
controller phases, target-side attempts, persistence-in-progress and concurrent
pairing availability windows are not exercised here. No production behavior,
allocation, retained capacity or wire format changes.

## Diagnostic mutations

Two temporary mutations were individually rejected by the focused test's
whole-schedule comparison:

1. Omitting the pairing refresh leaves `At(InstantMillis(119856))` cached after
   the matching attempt is gone; recomputation says `Idle`.
2. Returning unconditional `Idle` incorrectly disarms the unrelated attempt;
   recomputation retains `At(InstantMillis(119856))`.

Both mutations were removed before final verification. These are targeted manual
diagnostics, not a full mutation campaign or newly discovered production bugs.

## Verification

Passed on macOS arm64, using `CARGO_INCREMENTAL=0` for tests and clippy:

- `cargo test --locked -p prns-core stale_link_teardown_refreshes_only_its_controller_pairing_attempt`.
- `cargo test --locked -p prns-core engine::deadlines --quiet` (41 tests).
- `cargo test --locked --quiet`.
- `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core --all-targets -- -D warnings`.
- `cargo fmt --all -- --check` and `git diff --check`.

No mixed-runtime, firmware, Miri/ISA, physical-hardware or performance rerun is
claimed for this test-only slice.
