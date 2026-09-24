# Competing whole-response offers

Local macOS arm64 candidate based on `91ef84393`, 2026-09-26.

## Finding and scope

Following the simulator's sender-exclusion scenarios, a deterministic core-engine
fixture uses independent sender state with the same test link keys to emit an
authentic competing whole Resource advertisement. This deliberately bypasses the
normal sender's `LinkBusy` safeguard. Before the fix, the receiver requested the
competitor's data despite an admitted split response owning the same request.
This is a core-engine reproduction, not a mixed-runtime BLE injection.

Shared core now rejects whole offers for that link/request before size policy,
allocation or receipt ownership changes. Admission rechecks after deferred
application decisions. Queued offers are made immediately due and cancelled even
when transfer storage remains full or the queue deadline has passed. The typed
superseded outcome carries no request-failure settlement.

There are no new retained fields, capacities or wire formats. The existing
authenticated receiver-cancel path consumes fresh caller-supplied entropy.

## Exact assertions and remaining gap

Fresh offers declare 128, 257 or `u64::MAX` bytes. Each receives one authenticated
cancellation, settles the independent competitor as `RejectedByPeer`, preserves
the original request deadline, and allows exact original split completion.
Predicate coverage distinguishes different links, request IDs, split shapes and
unsolicited Resources.

The queued test explicitly constructs the ownership boundary: a whole offer waits
behind full transfer storage before a split assembly acquires the request. It
checks promotion-ready, still-full and expired states, cancellation without other
effects, unchanged request deadline and later original completion. Time advances
monotonically, including completion after the expired-queue check.

Temporarily bypassing the watchdog's superseded-offer check made the queued test
fail (no cancellation while storage remained full). Restoring it passes. The
fresh-offer test also failed before implementation with `ResourceRequest` where
`ResourceReceiverCancel` was expected.

Already-admitted whole transfers remain a completion-time arbitration gap. An
existing malformed-whole test now explicitly admits the whole transfer *before*
installing the later split owner, preserving its exact assertions about that
unfixed case. This slice does not claim whole/split overlap is fully solved.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- Focused `split_` and `competing_whole` core tests; full `prns-core` tests.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards` passed.
- Registered `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `embedded-miri-quick`, `embedded-isa-thumbv7em`, `embedded-isa-riscv32imac`
  and `embedded-isa-xtensa-esp32s3` suites passed. These existing Miri/ISA suites
  do not directly execute the new conflicting-peer tests.

The canonical `./tools/prns build embedded resources report --target t114`
passed without host Cargo environment overrides: image 624,284 bytes (+440),
static RAM 142,996 bytes (unchanged), RAM headroom 360 bytes (unchanged), modeled
stack reservation remainder 10,352 bytes (unchanged), flash headroom 141,668 bytes.
Modeled stack evidence retains its reported indirect-call/interrupt gaps.

`./tools/prns build embedded resources contracts --check` still fails on the
previously observed stale generated `tools/release/flasher_memory_contracts.py`;
that unrelated generated file is unchanged by this slice.

No physical-hardware, full board matrix, stock interoperability or benchmark run
is claimed. Doctor reported missing Renode and approximately 0.3 GiB free disk;
the available QEMU ISA suites and one canonical firmware target were exercised.
