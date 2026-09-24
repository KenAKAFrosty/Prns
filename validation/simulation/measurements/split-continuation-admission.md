# Split-continuation admission

Local macOS arm64 evidence, 2026-09-26, uncommitted candidate based on `f8c6700fb`.
This is shared-core evidence, not firmware or hardware qualification.

## Reproduction and correction

Two owner tests failed against the preceding code. A continuation could change
the original segment count and still trigger a pull. A queued continuation could
be promoted after its assembly disappeared, allocating buffers and claiming the
request without an assembly to finish. These are protocol/admission failures,
not evidence of historical user file loss or RF packet loss.

`IncomingAssemblies::fit` now takes the offered total segment count and compares
it to the stored count as well as the original hash and next index. It refuses
positions beyond completion and uses checked index arithmetic. Both advertisement
screening and actual admission apply that contract; the latter also covers queue
promotion and application-approved offers. No retained fields, buffers,
capacities or wire shapes change. The existing malformed-offer outcome remains
silent and does not settle a still-pending response request.

`receive/split_admission_tests/mod.rs` drives real encrypted Resource segments through
fixed-storage core engines. It proves unchanged response bytes and terminal RTT,
refusal of changed totals, successful recovery with a valid advertisement, bounded
queue promotion, no allocation or timeout claim for stale queued continuations,
and preservation of the current assembly. Queue tests deliberately inject removed,
replaced, advanced and changed-count assembly states while the transfer store is
full; they exercise the promotion boundary, not every real-world path that could
produce those states. Assembly unit/property tests cover count drift in either
direction, completed chains, unknown links/hashes and integer-limit positions.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`. Completed:

- `cargo test --locked -p prns-core routing::links::resources --lib` (260 tests).
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host split_admission_tests --lib`
  (two owner tests, without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- Registered suites `virtual-device-simulation` (including fourteen mixed
  Tokio/Embassy BLE scenarios), `bluetooth-auto-embassy`, `interop-large-request`,
  `interop-resource-rejection` and `interop-hopspot-remote-path`, via
  `python3 validation/run.py run --suite …`.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify` and the parent Rust repository guard.

The first workspace run failed in the unrelated flasher test
`offline_cache_rejects_symbolic_links`: after its safety assertion passed,
fixture cleanup returned `Directory not empty`. The isolated test and full
workspace rerun passed without source changes. The root cause of that transient
cleanup failure was not established. The first three interoperability invocations
were blocked by sandbox socket permissions; all passed when rerun with local
socket access. No full standalone runtime suites were rerun for this slice.

## Focused mutation audit

The zero-context diff of `assembly/core.rs` and `receive/gate.rs` selected 19
mutants with `cargo mutants --in-place --no-config -p prns-core --in-diff`,
`-f 'prns-core/src/routing/links/resources/{assembly/core,receive/gate}.rs'`,
`--no-shuffle --timeout 30 --build-timeout 180 -C=--locked` and
`-- --lib routing::links::resources`. `PROPTEST_DISABLE_FAILURE_PERSISTENCE=1`
prevented synthetic failures from creating regression files. Baseline passed;
15 mutants were caught, three generated `Default` replacements could not compile,
one survived and none timed out. Evidence is retained under
`validation-artifacts/split-continuation-admission-mutation/`.

The survivor changes `total_segment_count > 1` to `>= 1` in
`admit_accepted_resource`. The unchanged `segment_index > 1` condition and the
wire gate's `index <= total` invariant already imply `total > 1` for every
reachable continuation. Queue storage preserves both fields. This appears
equivalent for reachable admissions, not a missed behavioral refusal.
Fingerprint: `b38cf56df8015b6328c62aa2a0af0d629a4f79d746091615f7b1e683cf23ad02`.
It is recorded for human review, not accepted into global triage or excluded.
`mutation-shard-check` confirms complete evidence; `mutation-check` remains red
for that untriaged survivor. No production rewrite was made to improve the score.

After the audit, source restoration was checked against the pre-audit diff and
the Resource tests, no-default-features build, Clippy and formatting were rechecked.

## Coverage limits

Cumulative value-byte budgets, advertised stream totals, request-ID continuity,
overlapping admitted chains and cleanup between segments remain open. This slice
does not claim complete protection against inconsistent or dishonest split chains.
No firmware resource build, Miri/ISA, other native host platform, physical radio,
hardware or many-node scale checks have been run for this candidate.
