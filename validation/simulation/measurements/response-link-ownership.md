# Link-scoped response receipts

Local macOS arm64 evidence, 2026-09-26, candidate based on `663c3a2c0`.
This is shared-core evidence, not firmware or hardware qualification.

## Reproduction and correction

The old request lookup matched only the truncated request hash and receipt kind.
Two behavioral tests failed when that predicate was restored inside the new
link-parameter API: a packet authenticated on link A completed link B's pending
request with A's bytes, and a Resource offer from A allocated a transfer for B's
request. This was a controlled predicate reversion, not a pristine old checkout.
The fixtures supply the foreign request ID deliberately; they do not demonstrate
guessing a 128-bit ID, breaking link authentication or historical exploitation.

`Receipts::request_row_index` now requires both the owning `LinkId` and
`RequestId`. Public receipt reads, claims, timeout changes and settlements require
the link argument, without an ID-only compatibility alias. All repository callers
pass the packet, transfer or pending offer's link. This includes packet limits,
Resource admission, split/whole completion, cancellation and pending-offer expiry.
The receipt already stores its link; no retained fields, allocations, capacities
or wire formats were added. The scan remains linear; no speedup is claimed.

The new `request_link_tests.rs` and `response_link_tests.rs` modules are explicitly
`#[cfg(test)]`. They cover whole receipt-policy snapshots, both lookup-key
components, fixed and heap stores, generated distinct links, identical request
IDs on different links, unrelated send kinds and exact settlement. Engine tests
use two active links with different keys and real encrypted packets/Resource
offers. Wrong-link accepted-size and oversized responses leave the owner's
deadline intact; the owner then completes with the exact bytes and RTT. A late
Resource cancellation cannot settle the other link's identically named request
after the original receipt retires. Retirement is injected at the receipt seam;
the test does not claim to exercise every path that can retire a receipt.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`. Completed:

- `cargo test --locked -p prns-core link_tests --lib` (six tests).
- `cargo test --locked -p prns-core --lib` (2,010 passed, three ignored).
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host link_tests --lib`
  (six tests, without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- Registered suites `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `interop-large-request`, `interop-resource-rejection` and
  `interop-hopspot-remote-path`, via `python3 validation/run.py run --suite …`.
  The three interoperability suites ran with local socket access.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify` and
  `cargo run --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

## Focused mutation audit

`cargo mutants --in-place --no-config -p prns-core
-f prns-core/src/routing/delivery/receipts/core.rs --re request_row_index
--no-shuffle --timeout 30 --build-timeout 180
-o validation-artifacts/response-link-ownership-mutation -C=--locked -- --lib link_tests`
ran with `CARGO_INCREMENTAL=0 PROPTEST_DISABLE_FAILURE_PERSISTENCE=1`.
The six-test baseline passed; all five generated mutants were caught by assertion
failures, with no survivors, unviable builds or timeouts. These cover missing or
constant lookup results, disjunction instead of conjunction, and inverted request
hash comparison. The tool did not mutate the `matches!` link guard; the deliberate
old-predicate reproduction above separately exercises removal of that guard.

`mutation-shard-check` and `mutation-check` both passed against
`validation-artifacts/response-link-ownership-mutation/mutants.out/outcomes.json`.
The production diff was compared with its pre-audit copy to verify restoration.
Owner tests, the no-default-features build, focused Clippy and formatting were
then rechecked successfully.

## Coverage limits

Same-link split-chain correlation, overlapping admitted assemblies, cumulative
value-byte budgets, advertised stream totals and cleanup between segments remain
open in the [owner plan](../../../prns-core/plans/response-size-accounting.md).
The existing fallback to unsolicited Resource journals when a response receipt
has disappeared is unchanged. This slice prevents borrowing another link's
receipt; it does not redesign those orphaned-transfer semantics.

No firmware resource build, Miri/ISA, full applicable-host PR tier, other native
host platform, physical radio, hardware or many-node scale checks were run for
this candidate. Root/workspace tests do not imply separately rooted Cargo
workspaces passed. The previous continuation-admission mutation survivor remains
unwaived as recorded in [its evidence](split-continuation-admission.md).
