# Abandoned buffered waiters and completion-slot reuse

macOS arm64, based on `fca89ccd2`, 2026-09-27.

## Behavior under test

Dropping a buffered request future is not the same as expiring its protocol
receipt. This slice tests that distinction without changing either runtime's
cancellation policy.

An ordinary echo request starts, and its real response is dropped at the wire
gate. Once that exact loss is observed, `select!` drops the pending request
future. A segmented buffered file request then starts on the same link. At each
of the established three reception phases, the responder emits a new packet for
the abandoned request ID. That ID must differ from the replacement request's ID.

A concurrent buffered echo must succeed while the file stays pending. This
occupies both Embassy completion slots after the old waiter was dropped. Only
then is file loss stopped, and the exact file bytes and measured RTT must arrive.
The scenario reserves three test-local protocol receipts for the abandoned
request and two new requests, so receipt eviction cannot masquerade as waiter
cleanup. Completion capacity remains the ordinary two slots; no production
capacity is enlarged.

The adapters intentionally route the unawaited old response differently:

- Tokio retains its private completion entry until settlement, consumes the late
  response and emits no raw application response or settlement for it.
- Embassy's dropped waiter releases its slot immediately. The still-live
  protocol request can finish, but its whole response and one settlement now
  reach the application. The test checks the exact old request ID, value,
  command ID consistency, response evidence and elapsed RTT, then consumes that
  expected journal before fixture teardown.

Neither path may redirect old bytes into the new request. Both request slots are
subsequently reused concurrently again, and the existing same-link raw/buffered
recovery checks pass. Each runtime test runs three replacement phases and two
profile pairs, for twelve combinations. All radio, gate and observation cleanup
checks remain active.

## Diagnostic evidence and limits

Temporarily removing `RequestSlotGuard`'s release on drop made the Embassy test
fail with `Busy` on the concurrent echo instead of its exact expected reply.
The diagnostic stops on the first phase/profile; it is not a full mutation
campaign. The release was restored before final verification.

This is assurance for existing behavior, not a new cancellation implementation.
It does not prove immediate protocol cancellation, cancellation during request
admission, all buffered transfer phases at cancellation itself, or bounded Tokio
retention under indefinitely unresponsive peers. Cancellation here occurs while
waiting for a packet echo; the replacement is the segmented transfer. Native OS
Bluetooth stacks, physical RF, firmware memory and worker pools are not modeled
by this scenario. Production behavior, allocation layout and wire formats are
unchanged.

## Verification

Passed on macOS arm64; tests and clippy used `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble abandoned_waiter --quiet` (two scenarios, six combinations each).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet` (76 tests including observer/gate unit tests).
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware matrix, Miri/ISA, physical hardware, stock interoperability or
benchmark rerun is claimed for this test-only slice.
