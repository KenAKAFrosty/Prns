# Split-response failure settlement

Local macOS arm64 evidence, 2026-09-26, uncommitted candidate based on `cd05255e1`.
This is shared receiver/adapter evidence, not firmware or hardware qualification.

## Reproduction and correction

An encrypted, hash-verified split response advertised one request ID but enclosed
a different ID in its first segment. The old receiver silently omitted that
segment, delivered the tail and settled the request successfully. The regression
reproduced the complete incorrect result: only chunk 2, a success settlement and
no failure. This is a semantic delivery error after transport verification, not
evidence of RF packet loss or of corruption in users' historical files.

The shared receiver now propagates `TransferCorrupt` on both normal opening and
the resumed-decompression path, settles the request once and abandons the split
assembly. Late continuations and duplicate parts produce no successful delivery.
Matching envelopes retain their full response and measured RTT. Cancellation,
malformed metadata, invalid hashmaps and exhausted retries also release the failed
assembly. A failed whole transfer preserves an unrelated split assembly waiting
on the same link. No shipping buffers, retained state fields, capacities or wire
formats change; segmented response-size admission remains deliberately strict.

The opening tests feed real encrypted Resource advertisements, pulls and parts
between core engines. The decompression variants use synthetic compressed
candidates and explicitly resume the core with plaintext: they test the worker
completion contract, not the bzip2 codec. Tokio and Embassy adapter tests inject
the resulting journal contract, proving a typed failure discards buffered partial
content and the next complete request reuses its awaiter/slot. Direct journal
subscribers may have seen earlier chunks; those remain provisional until success.

Three initial owner regressions failed before the correction (false success and
stale assembly state).

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`. Completed:

- `cargo test --locked -p prns-core routing::links::resources --lib` (255 tests).
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host split_response_failures --lib`
  (six owner tests without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --target-dir target --lib`
  (253 tests).
- `cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --target-dir target --lib`
  (140 tests). Both new adapter tests also passed first with the
  `failed_split_response` filter. These packages are separate workspaces and are
  not implied by root workspace testing.
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- `cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --target-dir target --tests -- -D warnings`.
- `cargo clippy --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --target-dir target --tests -- -D warnings`.
- Registered suites `virtual-device-simulation` (including all fourteen mixed
  Tokio/Embassy BLE scenarios), `bluetooth-auto-embassy`, `interop-large-request`,
  `interop-resource-rejection`, and `interop-hopspot-remote-path`, via
  `python3 validation/run.py run --suite …`.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and the parent Rust repository guard.

## Focused mutation audit

`cargo mutants --in-place --no-config -p prns-core` selected the changed receiver
lines via `--in-diff` and `-f 'prns-core/src/routing/links/resources/receive/*.rs'`.
The completed run used `--no-shuffle --timeout 30 --build-timeout 180 -C=--locked`
and `-- --lib split_response_failures`, with
`PROPTEST_DISABLE_FAILURE_PERSISTENCE=1`. Its unmutated baseline passed; all seven
compiling mutants were caught, and seven were unviable because their generated
`Default` replacements are not implemented by the relevant types. No mutant was
missed or timed out. `mutation-shard-check` passed for the 14 outcomes in
`validation-artifacts/split-response-failures-mutation-focused/mutants.out/outcomes.json`.

An earlier run against the broader Resource test filter was interrupted: removing
transfer retirement made a watchdog loop without terminating. The owner test now
fails immediately on a second failure/settlement event; the focused rerun caught
that mutation promptly. The incomplete audit remains under
`validation-artifacts/split-response-failures-mutation/`; it is not passing evidence.
No production changes were made to improve the mutation score. Source restoration
was checked against the pre-audit diff, then the 255 Resource tests, six inline
owner tests and core/simulation Clippy check were rerun.

## Coverage limits

The next accounting slice still needs cumulative response-value budgets and
segmented stream-total validation. This slice does not claim complete protection
against every dishonest advertisement or split-chain inconsistency. Cleanup for
expiry between segments and pre-admission continuation refusal remains part of
that follow-up. No firmware resource build, Miri/ISA, other native host platform,
hardware, RF or many-node scale checks have been run for this candidate.
