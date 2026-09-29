# Split-position ownership

Local macOS arm64 evidence, 2026-09-26, candidate based on `9cfcf8e33`.

## Reproduction and correction

Two tests failed before the completion/cleanup correction. With two admitted
first segments on the same link, the older segment could publish bytes and
advance the replacement assembly. Cancelling the older transfer cleared the
replacement assembly. The reproduction already retained the new original-hash
field, but did not use it to change completion or cleanup; it was not a pristine
old checkout. These are controlled protocol failures, not evidence of historical
user file loss.

`IncomingResourceState` now retains the offered original hash, provided explicitly
to `IncomingResources::accept`. Normal opening and resumed decompression check
that hash, index and total count against the current assembly before emitting a
proof or data. Stale transfers fail as `TransferCorrupt`. Failure cleanup clears
only the assembly at the failed transfer's expected position. Advancement itself
also requires the original hash, index and count and refuses a stale position
without changing progress. The shared-core change applies to fixed and heap
storage consumers; no adapter-specific response logic or wire format changed.

`receive/split_ownership_tests.rs` is explicitly test-only. Real encrypted
Resource frames reproduce replacement on one link with distinct pending request
IDs. Tests cover normal completion, a parked decompression callback and sender
cancellation, then prove exact completion of the replacement response, terminal
RTT and retired state. The decompression test supplies worker output at the core
seam; it does not test a bzip2 codec. Assembly property tests compare the whole
position/progress state for expected, wrong-hash, wrong-index and changed-count
advancement, including integer-limit arithmetic and saturating byte totals.

## Verification

Host Cargo checks use `CARGO_INCREMENTAL=0`; canonical firmware builds do not
inherit that override. Completed:

- `cargo test --locked -p prns-core routing::links::resources --lib` (263 tests).
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host split_ownership_tests --lib`
  (two tests, without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- Registered suites `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `interop-large-request`, `interop-resource-rejection` and
  `interop-hopspot-remote-path`, via `python3 validation/run.py run --suite …`.
  Interoperability ran with local loopback socket access.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify` and
  `cargo run --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`.

## Memory and separate readiness findings

This adds a 32-byte identity field to every incoming transfer state, including
vacant fixed-table slots. It does not reduce buffers or capacities. The canonical
T114 report passed: 142,964 static section bytes plus four bytes of linker padding,
the unchanged 69,632-byte runtime-stack reservation and 392 bytes of remaining
RAM headroom. Flash headroom is 145,860 bytes. The stack assessment is within its
reservation with 10,352 bytes remaining; its reported missing-frame/call-graph
gaps remain, so this is not a complete worst-case stack proof. The report is at
`target/flash-artifacts/resources/configured/reports/t114.json`, based on this
candidate's production code before documentation updates. No before/after RAM
delta is claimed from older cached reports.

The initial resource invocation correctly refused an inherited
`CARGO_INCREMENTAL` override. It passed when rerun without build-semantic overrides;
no canonical flags or memory guards were relaxed.

`./tools/prns build embedded resources contracts --check` remains red for a
pre-existing generated-file mismatch. Regeneration adds only the existing
`muzi-base-duo` entry to `tools/release/flasher_memory_contracts.py`; its catalog
and generator inputs are unchanged by this slice. That unrelated generated edit
was inspected and reverted. `./tools/prns doctor embedded-assurance` also reports
the missing pinned Renode pilot; resource, Miri and ISA toolchains were present.
Neither finding was bypassed or presented as a passing check.

## Focused mutation audit

The zero-context diff of `assembly/core.rs` and `receive/conclude.rs` selected 27
mutants with `cargo mutants --in-place --no-config -p prns-core --in-diff`,
`-f 'prns-core/src/routing/links/resources/{assembly/core,receive/conclude}.rs'`,
`--no-shuffle --timeout 30 --build-timeout 180 -C=--locked` and
`-- --lib routing::links::resources`. The run used
`CARGO_INCREMENTAL=0 PROPTEST_DISABLE_FAILURE_PERSISTENCE=1`; its 263-test baseline
passed. Twenty-one mutants were caught, five generated `Default` replacements
could not compile, one survived and none timed out. Evidence is retained under
`validation-artifacts/split-position-ownership-mutation/`.

The survivor changes `state.total_segments > 1` to `>= 1` in
`settle_failed_resource_claim`. The sole production assembly-opening site already
requires a count greater than one, and the unchanged `split_segment_fit` requires
the offered count to equal that stored count. Therefore the added whole-transfer
case cannot clear a reachable split assembly. This appears equivalent on reachable
paths, not a missed cleanup refusal. Fingerprint:
`b8d3ecc0582d32aa65438fb580f31e3ffc081ef9096ed61891abcce9236239d6`.
It is recorded for human review, not globally accepted or excluded. No production
rewrite was made for the mutation score. `mutation-shard-check` confirms complete
evidence; `mutation-check` remains red for the untriaged survivor.

The source diff matched its pre-audit copy after restoration.
Resource tests, the no-default-features build, focused Clippy and formatting were
then rechecked successfully.

## Coverage limits

The assembly still does not bind a request ID/correlation kind across segments.
Overlapping chains naming the same request can therefore still interfere with
its receipt. Cumulative value budgets, advertised stream totals, receipt expiry
between segments and pre-admission cleanup also remain open in the
[owner plan](../../../prns-core/plans/response-size-accounting.md).

No full firmware resource matrix, Miri/ISA execution, Renode, other host platform,
hardware or many-node scale checks were run for this slice. Root/workspace tests
do not imply separately rooted Cargo workspaces passed.
